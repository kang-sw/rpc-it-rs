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
    Codec, ParseMessage, ResponseError, UserData,
};

use super::{
    error::{SendMsgError, SendResponseError, TryRecvError, TrySendMsgError, TrySendResponseError},
    RpcCore,
};

/// Handles error during receiving inbound messages inside runner.
///
/// By result of handled error, it can determine whether to continue receiving inbound(i.e. maintain
/// the connection) or not.
pub trait ReceiveErrorHandler<T: UserData>: 'static + Send {
    /// Error during decoding received inbound message.
    fn on_inbound_decode_error(
        &mut self,
        user_data: &T,
        unparsed_frame: &[u8],
        error_type: DecodeError,
    ) -> Result<(), ReadRunnerError> {
        Ok(())
    }
}

/// Default implementation for any type of user data. It ignores every error.
impl<T> ReceiveErrorHandler<T> for () where T: UserData {}

/// A receiver which deals with inbound notifies / requests.
#[derive(Debug)]
pub struct Receiver<U> {
    pub(super) context: Arc<dyn RpcCore<U>>,

    /// Even if all receivers are dropped, the background task possibly retain if there's any
    /// present [`crate::RequestSender`] instance.
    pub(super) channel: mpsc::Receiver<InboundDelivery>,
}

// ==== impl:Receiver ====

impl<U> Receiver<U>
where
    U: UserData,
{
    pub fn user_data(&self) -> &U {
        self.context.user_data()
    }

    pub fn codec(&self) -> &dyn Codec {
        self.context.codec()
    }

    pub fn cloned_codec(&self) -> Arc<dyn Codec> {
        self.context.clone().self_as_codec()
    }

    /// Receive an inbound message from remote.
    pub async fn recv(&self) -> Option<Inbound<'_, U>> {
        self.channel
            .recv()
            .await
            .map(|inner| Inbound {
                owner: Cow::Borrowed(&self.context),
                inner,
            })
            .ok()
    }

    /// Tries to receive an inbound message from remote.
    pub async fn try_recv(&self) -> Result<Inbound<'_, U>, TryRecvError> {
        self.channel
            .try_recv()
            .map(|inner| Inbound {
                owner: Cow::Borrowed(&self.context),
                inner,
            })
            .map_err(|e| match e {
                mpsc::TryRecvError::Empty => TryRecvError::Empty,
                mpsc::TryRecvError::Closed => TryRecvError::Closed,
            })
    }

    /// Change this channel into a stream. It'll return self-contained references of inbound. It'll
    /// a bit more inefficient than calling `recv` since it clones single [`Arc`] for each inbound
    /// message.
    pub fn into_stream(self) -> impl futures::Stream<Item = Inbound<'static, U>> {
        let Self { channel, context } = self;

        channel.map(move |item| Inbound {
            owner: Cow::Owned(context.clone()),
            inner: item,
        })
    }

    /// Closes rx channel.
    pub fn shutdown_reader(self) {
        self.channel.close();
        self.context.shutdown_rx_channel();
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
pub struct Inbound<'a, U>
where
    U: UserData,
{
    owner: Cow<'a, Arc<dyn RpcCore<U>>>,
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

impl<'a, U> Inbound<'a, U>
where
    U: UserData,
{
    pub fn user_data(&self) -> &U {
        self.owner.user_data()
    }

    pub fn codec(&self) -> &dyn Codec {
        self.owner.codec()
    }

    pub fn cloned_codec(&self) -> Arc<dyn Codec> {
        (*self.owner).clone().self_as_codec()
    }

    /// Consumes this struct and returns an owned version of it.
    ///
    /// If what you only have is a reference to this struct, you can use [`Inbound::clone_notify`]
    ///
    /// ```no_run
    /// use rpc_it::Inbound;
    ///
    /// fn elevate_inbound<'a>(ib: &mut Inbound<'a, ()>) {
    ///   let owned = ib.take().into_owned();
    /// }
    /// ```
    pub fn into_owned(mut self) -> Inbound<'static, U> {
        Inbound {
            owner: Cow::Owned(Arc::clone(&self.owner)),
            inner: take(&mut self.inner),
        }
    }

    /// Retrieves message out of the reference.
    pub fn take(&mut self) -> Inbound<'_, U> {
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
    /// If you want to "retrieve" the request out of this struct, as long as you have a mutable
    /// reference to self, you can use [`Inbound::take`] method to retrieve the request out of this
    /// struct as this defines [`Default`] implementation.
    pub fn clone_notify(&self) -> Inbound<'a, U> {
        Inbound {
            owner: self.owner.clone(),
            inner: InboundDelivery {
                buffer: self.inner.buffer.clone(),
                method: self.inner.method,
                payload: self.inner.payload,
                req_id: 0,
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
    /// use rpc_it::{Inbound, ResponseError, BytesMut};
    ///
    /// async fn response_examples(b: &mut BytesMut, ib: &mut Inbound<'_, ()>) {
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
        type R<'a> = EncodeResponsePayload<'a>;

        let encoded = match &payload {
            Ok(obj) => R::Ok(obj),
            Err((ResponseError::Unknown, Some(ref obj))) => R::ErrObjectOnly(obj),
            Err((errc, Some(ref obj))) => R::Err(*errc, obj),
            Err((errc, None)) => R::ErrCodeOnly(*errc),
        };

        Some(self.owner.codec().encode_response(req_id, encoded, buf))
    }

    /// Drops the request without sending any response. This prevents drop-guard automatically
    /// responding [`ResponseError::Unhandled`].
    pub fn drop_request(&self) {
        // Simply take the request range, and drop it.
        let _ = self.atomic_take_req_range();
    }
}

impl<'a, U> Drop for Inbound<'a, U>
where
    U: UserData,
{
    fn drop(&mut self) {
        if let Some(req_id) = self.atomic_take_req_range() {
            let req_id = self.retrieve_req_id(req_id);
            self.owner.on_request_unhandled(req_id);
        }
    }
}

impl<'a, U> ParseMessage for Inbound<'a, U>
where
    U: UserData,
{
    fn codec_payload_pair(&self) -> (&dyn Codec, &[u8]) {
        (self.owner.codec(), self.payload_bytes())
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
