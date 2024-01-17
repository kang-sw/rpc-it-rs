use std::{
    borrow::Cow,
    mem::{take, transmute},
    sync::Arc,
};

use bytes::{Bytes, BytesMut};
use futures::StreamExt;

use crate::{
    codec::{
        error::{DecodeError, EncodeError},
        EncodeResponsePayload,
    },
    defs::{
        AtomicLongSizeType, LongSizeType, NonZeroRangeType, NonzeroSizeType, RangeType, SizeType,
    },
    error::ReadRunnerError,
    rpc::DeferredDirective,
    Codec, NotifySender, ParseMessage, ResponseError, UserData,
};

use super::{
    core::RpcCore,
    error::{SendMsgError, SendResponseError, TryRecvError, TrySendMsgError, TrySendResponseError},
};

/// Handles error during receiving inbound messages inside runner.
///
/// By result of handled error, it can determine whether to continue receiving inbound(i.e. maintain
/// the connection) or not.
pub trait ReceiveErrorHandler<U: UserData, C: Codec>: 'static + Send {
    /// Error during decoding received inbound message.
    fn on_inbound_decode_error(
        &mut self,
        user_data: &U,
        codec: &C,
        unparsed_frame: &[u8],
        error_type: DecodeError,
    ) -> Result<(), ReadRunnerError> {
        let _ = (user_data, unparsed_frame, error_type, codec);
        Ok(())
    }

    /// Called when response is received, when the request feature is not enabled, which is ideally
    /// impossible to happen; You can treat this as remote protocol violation to disconnect the
    /// remote peer, or just ignore it.
    fn on_impossible_response_inbound(&mut self, user_data: &U) -> Result<(), ReadRunnerError> {
        let _ = user_data;
        Ok(())
    }

    /// Called when the inbound message is not a valid UTF-8 string.
    fn on_inbound_method_invalid_utf8(
        &mut self,
        user_data: &U,
        method_bytes: &[u8],
    ) -> Result<(), ReadRunnerError> {
        let _ = (user_data, method_bytes);
        Ok(())
    }

    /// When all receiver channels are dropped but since there are still [`crate::RequestSender`]
    /// instances, the background receiver task is still able to handle inbound messages to deal
    /// with responses from remote peer.
    ///
    /// This method is called when the background receiver task receives a inbound message other
    /// than response, but there are no [`crate::Receiver`] instances to handle it.
    fn on_unhandled_notify_or_request(
        &mut self,
        user_data: &U,
        inbound: Inbound<U, C>,
    ) -> Result<(), ReadRunnerError> {
        let _ = (user_data, inbound);
        Ok(())
    }
}

/// Default implementation for any type of user data. It ignores every error.
impl<U, C> ReceiveErrorHandler<U, C> for ()
where
    U: UserData,
    C: Codec,
{
}

/// A receiver which deals with inbound notifies / requests.
#[derive(Debug)]
pub struct Receiver<U: UserData, C: Codec> {
    pub(super) context: Arc<RpcCore<U, C>>,

    /// Even if all receivers are dropped, the background task possibly retain if there's any
    /// present [`crate::RequestSender`] instance.
    pub(super) channel: mpsc::Receiver<InboundDelivery>,
}

// ==== impl:Receiver ====

impl<U, C> Clone for Receiver<U, C>
where
    U: UserData,
    C: Codec,
{
    fn clone(&self) -> Self {
        Self {
            context: Arc::clone(&self.context),
            channel: self.channel.clone(),
        }
    }
}

impl<U, C> Receiver<U, C>
where
    U: UserData,
    C: Codec,
{
    pub fn user_data(&self) -> &U {
        self.context.user_data()
    }

    pub fn codec(&self) -> &C {
        &self.context.codec
    }

    /// Receive an inbound message from remote.
    pub async fn recv(&self) -> Option<Inbound<'_, U, C>> {
        self.channel
            .recv()
            .await
            .map(|inner| Inbound {
                owner: Cow::Borrowed(&self.context),
                inner,
            })
            .ok()
    }

    /// Creates a new notify channel from this receiver.
    pub fn notify_sender(&self) -> crate::NotifySender<U, C> {
        crate::NotifySender {
            context: Arc::clone(&self.context),
        }
    }

    /// Tries to receive an inbound message from remote.
    pub async fn try_recv(&self) -> Result<Inbound<'_, U, C>, TryRecvError> {
        self.channel
            .try_recv()
            .map(|inner| Inbound::new(Cow::Borrowed(&self.context), inner))
            .map_err(|e| match e {
                mpsc::TryRecvError::Empty => TryRecvError::Empty,
                mpsc::TryRecvError::Closed => TryRecvError::Closed,
            })
    }

    /// Change this channel into a stream. It'll return self-contained references of inbound. It'll
    /// a bit more inefficient than calling `recv` since it clones single [`Arc`] for each inbound
    /// message.
    pub fn into_stream(self) -> impl futures::Stream<Item = Inbound<'static, U, C>> {
        let Self { channel, context } = self;

        channel.map(move |item| Inbound::new(Cow::Owned(context.clone()), item))
    }

    /// Closes inbound channel. Except for messages that were already pushed into the channel, no
    /// additional messages will be pushed into the channel. If there's any alive senders that
    /// can send request message, the background receiver task will be kept alive. Otherwise, it'll
    /// be dropped on next message receive.
    ///
    /// To immediately close the background receiver task, all handles need to be dropped.
    pub fn close_inbound_channel(self) {
        self.channel.close();
    }
}

// ========================================================== Inbound Delivery ===|

impl InboundDelivery {
    pub(crate) fn new_request(
        buffer: Bytes,
        method: RangeType,
        payload: RangeType,
        req_id: NonZeroRangeType,
    ) -> Self {
        Self {
            buffer,
            method,
            payload,
            req_id: req_id_to_inner(Some(req_id)),
        }
    }

    pub(crate) fn new_notify(buffer: Bytes, method: RangeType, payload: RangeType) -> Self {
        Self {
            buffer,
            method,
            payload,
            req_id: 0,
        }
    }
}

fn req_id_to_inner(range: Option<NonZeroRangeType>) -> LongSizeType {
    // SAFETY: Just byte mucking
    unsafe {
        transmute(
            range
                .map(|range| [range.begin(), range.end().get()])
                .unwrap_or_default(),
        )
    }
}

// ========================================================== Inbound ===|

/// A inbound message that was received from remote. It is either a notification or a request.
pub struct Inbound<'a, U, C>
where
    U: UserData,
    C: Codec,
{
    owner: Cow<'a, Arc<RpcCore<U, C>>>,
    inner: InboundDelivery,
}

/// An inbound message delivered from the background receiver.
#[derive(Default)]
pub(crate) struct InboundDelivery {
    buffer: Bytes,
    method: RangeType,
    payload: RangeType,

    // Start / End index of request ID in the buffer. If value is 0, it's a notification.
    req_id: LongSizeType,
}

/// A type of response error that can be sent by [`Inbound::response`].
pub struct ResponsePayload<T: serde::Serialize>(Result<T, (ResponseError, Option<T>)>);

// ==== impl:Inbound ====

impl<'a, U, C> Inbound<'a, U, C>
where
    U: UserData,
    C: Codec,
{
    pub(super) fn new(owner: Cow<'a, Arc<RpcCore<U, C>>>, inner: InboundDelivery) -> Self {
        Self { owner, inner }
    }

    pub fn user_data(&self) -> &U {
        self.owner.user_data()
    }

    pub fn codec(&self) -> &C {
        &self.owner.codec
    }

    /// Creates a new notify channel from this inbound message. It is not guaranteed that the writer
    /// channel exist or not(i.e. handle may not be valid).
    pub fn create_sender_handle(&self) -> NotifySender<U, C> {
        NotifySender {
            context: Arc::clone(&self.owner),
        }
    }

    /// Consumes this struct and returns an owned version of it.
    ///
    /// If what you only have is a reference to this struct, you can use [`Inbound::clone_notify`]
    ///
    /// ```
    /// use rpc_it::Inbound;
    ///
    /// fn elevate_inbound<'a, C: rpc_it::Codec>(ib: &mut Inbound<'a, (), C>) {
    ///   let owned = ib.take().into_owned();
    /// }
    /// ```
    pub fn into_owned(mut self) -> Inbound<'static, U, C> {
        Inbound {
            owner: Cow::Owned(Arc::clone(&self.owner)),
            inner: take(&mut self.inner),
        }
    }

    /// Retrieves message out of the reference. Remaining reference will be invalidated.
    pub fn take(&mut self) -> Inbound<'_, U, C> {
        let req_id = self.atomic_take_req_range();
        Inbound {
            owner: Cow::Borrowed(&self.owner),
            inner: InboundDelivery {
                buffer: take(&mut self.inner.buffer),
                method: take(&mut self.inner.method),
                payload: take(&mut self.inner.payload),
                req_id: req_id_to_inner(req_id),
            },
        }
    }

    fn payload_bytes(&self) -> &[u8] {
        &self.inner.buffer[self.inner.payload.range()]
    }

    /// Clones the notify part of the inbound message. Since response can be sent only once, this
    /// struct does not define generic method for completely cloning the internal state.
    ///
    /// If `task_request` is true, it'll retrieve out the request ownership from the inbound
    /// message.
    pub fn clone_message(&self, take_request: bool) -> Inbound<'a, U, C> {
        Inbound {
            owner: self.owner.clone(),
            inner: InboundDelivery {
                buffer: self.inner.buffer.clone(),
                method: self.inner.method,
                payload: self.inner.payload,
                req_id: req_id_to_inner(if take_request {
                    self.atomic_take_req_range()
                } else {
                    None
                }),
            },
        }
    }

    /// Retrieve request atomicly.
    fn atomic_take_req_range(&self) -> Option<NonZeroRangeType> {
        // SAFETY: Just byte mucking
        let [begin, end] = unsafe {
            let ptr = &self.inner.req_id as *const _ as *mut _;
            let atomic = AtomicLongSizeType::from_ptr(ptr);
            let value = atomic.swap(0, std::sync::atomic::Ordering::Acquire);

            // The value was
            transmute::<_, [SizeType; 2]>(value)
        };

        NonzeroSizeType::new(end).map(|x| NonZeroRangeType::new(begin, x))
    }

    fn retrieve_req_id(&self, id_buf_range: NonZeroRangeType) -> &[u8] {
        &self.inner.buffer[id_buf_range.range()]
    }

    /// Check if this message is still a request, that can send valid response. This will return
    /// false if the message is already responded, or it's a notification at first place.
    pub fn is_request(&self) -> bool {
        self.inner.req_id != 0
    }

    /// A shorthand for unwrapping result [`Inbound::parse_method`].
    ///
    /// # Panics
    ///
    /// Panics if method name is not a valid UTF-8 string.
    pub fn method(&self) -> &str {
        let bytes = &self.inner.buffer[self.inner.method.range()];

        // If it's not UTF-8 string, it's crate logic error.
        debug_assert!(std::str::from_utf8(bytes).is_ok());

        // SAFETY:
        // * The receiver task firstly verifies if the method name is valid UTF-8 string.
        // * Thus, in this context, the method name bytes is guaranteed to be valid UTF-8 string.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }

    /// Response handle result.
    ///
    /// # Usage
    ///
    /// ```no_run
    /// use rpc_it::{Inbound, ResponseError, BytesMut, Codec};
    ///
    /// async fn response_examples<C: Codec>(b: &mut BytesMut, ib: &mut Inbound<'_, (), C>) {
    ///   // This returns plain ok response, with parameter "hello, world!"
    ///   ib.response(b, Ok("hello, world!"));
    ///
    ///   // This returns predefined error code `ResponseError::MethodNotFound`
    ///   ib.response(b, ResponseError::MethodNotFound);
    ///
    ///   // This will serialize the error object as-is, without specifying `ResponseError`.
    ///   //
    ///   // However, the actual implementation of error serialization is up to the codec.
    ///   ib.response(b, Err("Ta-Da!"));
    ///
    ///   // This is identical with the above example.
    ///   ib.response(b, (ResponseError::Unknown, "Ta-Da!"));
    ///
    ///   // This will serialize the error object in codec-custom way.
    ///   ib.response(b, (ResponseError::MethodNotFound, ib.method()));
    ///
    ///   // NOTE: In practice, this method panics since we can send response ONLY ONCE!
    /// }
    /// ```
    pub async fn response<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        result: impl Into<ResponsePayload<T>>,
    ) -> Result<(), SendResponseError> {
        self.encode_response(buf, result)
            .ok_or(SendResponseError::InboundNotRequest)?
            .map_err(SendMsgError::EncodeFailed)?;

        self.owner
            .tx_deferred()
            .ok_or(SendMsgError::ChannelClosed)?
            .send(DeferredDirective::WriteMsg(buf.split().freeze()))
            .await
            .map_err(|_| SendMsgError::ChannelClosed.into())
    }

    /// Try to send response.
    ///
    /// # Usage
    ///
    /// See [`Inbound::response`].
    pub fn try_response<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        result: impl Into<ResponsePayload<T>>,
    ) -> Result<(), TrySendResponseError> {
        self.encode_response(buf, result)
            .ok_or(TrySendResponseError::InboundNotRequest)?
            .map_err(TrySendMsgError::EncodeFailed)?;

        self.owner
            .tx_deferred()
            .ok_or(TrySendMsgError::ChannelClosed)?
            .try_send(DeferredDirective::WriteMsg(buf.split().freeze()))
            .map_err(|e| {
                match e {
                    mpsc::TrySendError::Full(_) => TrySendMsgError::ChannelAtCapacity,
                    mpsc::TrySendError::Closed(_) => TrySendMsgError::ChannelClosed,
                }
                .into()
            })
    }

    fn encode_response<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        result: impl Into<ResponsePayload<T>>,
    ) -> Option<Result<(), EncodeError>> {
        let req_range = self.atomic_take_req_range()?;
        let req_id = self.retrieve_req_id(req_range);

        let payload = result.into().0;
        type R<'a, S> = EncodeResponsePayload<'a, S>;

        let encoded = match &payload {
            Ok(obj) => R::<T>::Ok(obj),
            Err((ResponseError::Unknown, Some(ref obj))) => R::<T>::ErrObjectOnly(obj),
            Err((errc, Some(ref obj))) => R::<T>::Err(*errc, obj),
            Err((errc, None)) => R::<T>::ErrCodeOnly(*errc),
        };

        Some(self.owner.codec.encode_response(req_id, encoded, buf))
    }

    /// Drops the request without sending any response. This function is used to override the
    /// default behavior of the inbound request handling.
    ///
    /// By default, if an inbound request is dropped without a response, the system automatically
    /// sends a [`ResponseError::Unhandled`] message to the remote client. This default mechanism
    /// helps minimize the waiting time for the remote side, which might otherwise have to handle
    /// timeouts for unhandled requests.
    ///
    /// However, there might be situations where you want to drop a request without sending any
    /// response back. In such scenarios, this method can be explicitly called. It ensures that the
    /// request is dropped silently, without triggering the automatic [`ResponseError::Unhandled`]
    /// response from the drop guard.
    pub fn drop_request(&self) {
        // Simply take the request range, and drop it.
        let _ = self.atomic_take_req_range();
    }
}

impl InboundDelivery {
    pub(crate) fn method_bytes(&self) -> &[u8] {
        &self.buffer[self.method.range()]
    }
}

impl<'a, U, C> Drop for Inbound<'a, U, C>
where
    U: UserData,
    C: Codec,
{
    fn drop(&mut self) {
        // Sends 'unhandled' message to remote if it's still an unhandled request message until it's
        // being dropped. This is default behavior to prevent reducing remote side's request timeout
        // handling overhead.
        if self.is_request() {
            self.try_response(&mut BytesMut::new(), ResponseError::Unhandled)
                .ok();
        }
    }
}

impl<'a, U, C> ParseMessage<C> for Inbound<'a, U, C>
where
    U: UserData,
    C: Codec,
{
    fn codec_payload_pair(&self) -> (&C, &[u8]) {
        (&self.owner.codec, self.payload_bytes())
    }
}

// ==== Errors ====

impl From<ResponseError> for ResponsePayload<()> {
    fn from(code: ResponseError) -> Self {
        Self(Err((code, None)))
    }
}

impl<T> From<Result<T, T>> for ResponsePayload<T>
where
    T: serde::Serialize,
{
    fn from(result: Result<T, T>) -> Self {
        match result {
            Ok(obj) => Self(Ok(obj)),
            Err(obj) => Self(Err((ResponseError::Unknown, Some(obj)))),
        }
    }
}

impl<T> From<(ResponseError, T)> for ResponsePayload<T>
where
    T: serde::Serialize,
{
    fn from((code, obj): (ResponseError, T)) -> Self {
        Self(Err((code, Some(obj))))
    }
}
