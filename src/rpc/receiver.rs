use std::{
    mem::{take, transmute},
    pin::Pin,
    sync::Arc,
    task::Poll,
};

use bytes::{Bytes, BytesMut};
use futures::{Future, FutureExt};
use thiserror::Error;

use crate::{
    codec::{error::EncodeError, EncodeResponsePayload},
    defs::{
        AtomicLongSizeType, LongSizeType, NonZeroRangeType, NonzeroSizeType, RangeType, SizeType,
    },
    rpc::WriterDirective,
    AsyncFrameRead, Codec, NotifySender, ParseMessage, RequestSender, ResponseError,
};

use super::{
    core::RpcCore,
    error::{SendMsgError, SendResponseError, TrySendMsgError, TrySendResponseError},
    Config, PreparedPacket, ReadRunnerExitType, ResponseReceiveError,
};

pub use rx_inner::Error as ReceiveError;

/// A receiver task that receives inbound messages from remote.
///
///
#[derive(Debug)]
pub struct Receiver<R: Config, Rx> {
    core: Option<Arc<RpcCore<R>>>,

    /// Even if all receivers are dropped, the background task possibly retain if there's any
    /// present [`crate::RequestSender`] instance.
    read: Rx,

    /// Internal scratch buffer
    scratch: BytesMut,
}

// ==== impl:Receiver ====

impl<R: Config, Rx: AsyncFrameRead> Receiver<R, Rx> {
    pub(super) fn new(core: Arc<RpcCore<R>>, read: Rx) -> Self {
        Self {
            core: Some(core),
            read,
            scratch: Default::default(),
        }
    }

    fn core(&self) -> &Arc<RpcCore<R>> {
        // SAFETY: Core is always valid.
        unsafe { self.core.as_ref().unwrap_unchecked() }
    }

    pub fn user_data(&self) -> &R::UserData {
        self.core().user_data()
    }

    pub fn codec(&self) -> &R::Codec {
        &self.core().codec
    }

    /// Receive an inbound message from remote.
    pub async fn recv(&mut self) -> Result<Inbound<R>, ReceiveError<R::Codec>> {
        // SAFETY: We won't move the memory within visible scope.
        let mut read = unsafe { Pin::new_unchecked(&mut self.read) };

        // As remote response and inbound message are both sent via the same channel, we need to
        // handle both cases. Response is handled internally, and inbound message will be returned
        // to caller.
        loop {
            let inbound = read.as_mut().next().await?.ok_or(ReceiveError::Eof)?;

            // SAFETY: Core is always valid unless we call `into_response_only_task`
            let core = unsafe { self.core.as_ref().unwrap_unchecked() };
            let task_rx = rx_inner::handle_inbound_once(core, inbound, &mut self.scratch);

            if let Some(rx) = task_rx.await? {
                break Ok(rx);
            }
        }
    }

    /// Creates a new notify channel from this receiver.
    ///
    /// This does not guarantee that the writer channel exist or not(i.e. handle may not be valid).
    ///
    /// If you need to send request, try upgrade returned handle to request sender.
    pub fn create_notify_sender(&self) -> crate::NotifySender<R> {
        crate::NotifySender {
            context: Arc::clone(self.core()),
        }
    }

    /// Closes inbound channel. Except for messages that were already pushed into the channel, no
    /// additional messages will be pushed into the channel. If there's any alive senders that
    /// can send request message, the background receiver task will be kept alive. Otherwise, it'll
    /// be dropped on next message receive.
    ///
    /// To immediately close the background receiver task, all handles need to be dropped.
    pub async fn into_weak_runner_task(
        mut self,
        on_inbound: impl Fn(Result<Inbound<R>, ReceiveError<R::Codec>>),
    ) -> std::io::Result<ReadRunnerExitType> {
        // SAFETY: We won't move the memory within visible scope.
        let mut read = unsafe { Pin::new_unchecked(&mut self.read) };

        // Downgrade the core to weak reference; let the sender handles control the lifetime of
        // background task.
        let wctx = Arc::downgrade(&self.core.take().unwrap());

        let task_receive_loop = async {
            loop {
                let Some(inbound) = read.as_mut().next().await? else {
                    return Ok(ReadRunnerExitType::Eof);
                };

                let Some(core) = wctx.upgrade() else {
                    return Ok(ReadRunnerExitType::AllHandleDropped);
                };

                if let Some(ib) = rx_inner::handle_inbound_once(&core, inbound, &mut self.scratch)
                    .await
                    .transpose()
                {
                    on_inbound(ib);
                }
            }
        };

        let task_signal_all_handle_dropped = std::future::poll_fn(|cx| {
            let Some(context) = wctx.upgrade() else {
                return Poll::Ready(());
            };

            context.register_wake_on_drop(cx.waker());
            Poll::Pending
        });

        futures::select! {
            result = task_receive_loop.fuse() => {
                result
            }
            _ = task_signal_all_handle_dropped.fuse() => {
                Ok(ReadRunnerExitType::AllHandleDropped)
            }
        }
    }

    // ==== Internals ====
}

impl<R: Config, Rx> Drop for Receiver<R, Rx> {
    fn drop(&mut self) {
        if let Some(reqs) = self.core.as_ref().and_then(|x| x.request_context()) {
            // As receiver is being dropped, outbound request is no longer possible.
            reqs.mark_expired();
        }
    }
}

mod rx_inner {
    use std::{ops::Range, str::Utf8Error, sync::Arc};

    use bytes::{Bytes, BytesMut};

    use crate::{
        codec::error::DecodeError,
        defs::{NonZeroRangeType, RangeType},
        rpc::{core::RpcCore, make_response, ErrorResponse, InboundInner},
        Codec, Config, Inbound, Response,
    };

    #[derive(thiserror::Error, Debug)]
    pub enum Error<C: Codec> {
        #[error("End of file")]
        Eof,

        #[error("IO error from channel. Channel highly likely to be closed. \n -- Error: {0}")]
        IoError(#[from] std::io::Error),

        #[error("Failed to decode inbound")]
        DecodeFailed(DecodeError, Bytes),

        #[error("Unexpected response error")]
        UnhandledResponse(Result<Response<C>, ErrorResponse<C>>),

        #[error("non-utf8 method name")]
        NonUtf8MethodName(Utf8Error, Bytes, Range<usize>),
    }

    // ==== Impls ====

    pub(super) async fn handle_inbound_once<R: Config>(
        core: &Arc<RpcCore<R>>,
        mut ib: Bytes,
        scratch: &mut BytesMut,
    ) -> Result<Option<Inbound<R>>, Error<R::Codec>> {
        let frame_type = match core.codec.decode_inbound(scratch, &mut ib) {
            Ok(x) => x,
            Err(e) => return Err(Error::DecodeFailed(e, ib)),
        };

        use crate::codec::InboundFrameType as IFT;
        let inbound = match frame_type {
            IFT::Request {
                req_id_raw: std::ops::Range { start, end },
                method,
                params,
            } => InboundInner::new_request(
                ib,
                method.into(),
                params.into(),
                NonZeroRangeType::new(start, end.try_into().unwrap()),
            ),
            IFT::Notify { method, params } => {
                InboundInner::new_notify(ib, method.into(), params.into())
            }
            IFT::Response {
                req_id,
                errc,
                payload,
            } => {
                let payload = ib.slice(RangeType::from(payload).range());

                let Some(reqs) = core.request_context() else {
                    return Err(Error::UnhandledResponse(make_response(
                        core.codec.clone(),
                        payload,
                        errc,
                    )));
                };

                if let Err(payload) = reqs.set_response(req_id, payload, errc) {
                    return Err(Error::UnhandledResponse(make_response(
                        core.codec.clone(),
                        payload,
                        errc,
                    )));
                };

                // For request handling; We don't need to return inbound message.
                return Ok(None);
            }
        };

        if let Err(e) = std::str::from_utf8(inbound.method_bytes()) {
            return Err(Error::NonUtf8MethodName(
                e,
                inbound.buffer,
                inbound.method.range(),
            ));
        }

        Ok(Some(Inbound::new(core.clone(), inbound)))
    }
}

// ========================================================== Inbound Delivery ===|

impl InboundInner {
    fn new_request(
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

    fn new_notify(buffer: Bytes, method: RangeType, payload: RangeType) -> Self {
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
pub struct Inbound<R: Config> {
    owner: Arc<RpcCore<R>>,
    inner: InboundInner,
}

/// An inbound message delivered from the background receiver.
#[derive(Default)]
pub(crate) struct InboundInner {
    buffer: Bytes,
    method: RangeType,
    payload: RangeType,

    // Start / End index of request ID in the buffer. If value is 0, it's a notification.
    req_id: LongSizeType,
}

/// A type of response error that can be sent by [`Inbound::response`].
pub struct ResponsePayload<T: serde::Serialize>(Result<T, (ResponseError, Option<T>)>);

// ==== impl:Inbound ====

impl<R: Config> Inbound<R> {
    pub(super) fn new(owner: Arc<RpcCore<R>>, inner: InboundInner) -> Self {
        Self { owner, inner }
    }

    pub fn user_data(&self) -> &R::UserData {
        self.owner.user_data()
    }

    pub fn codec(&self) -> &R::Codec {
        &self.owner.codec
    }

    /// Gets request ID of this inbound message. As request ID can be taken from constant reference,
    /// you only can get the request ID through mutable reference.
    pub fn request_id(&mut self) -> Option<&[u8]> {
        Self::to_req_range(self.inner.req_id).map(|x| self.retrieve_req_id(x))
    }

    /// Creates a new notify channel from this inbound message. It is not guaranteed that the writer
    /// channel exist or not(i.e. handle may not be valid).
    pub fn create_sender_handle(&self) -> NotifySender<R> {
        NotifySender {
            context: Arc::clone(&self.owner),
        }
    }

    /// Retrieves message out of the reference. Remaining reference will be invalidated.
    pub fn take(&mut self) -> Inbound<R> {
        let req_id = self.atomic_take_req_range();
        Inbound {
            owner: self.owner.clone(),
            inner: InboundInner {
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
    pub fn clone_message(&self, take_request: bool) -> Inbound<R> {
        Inbound {
            owner: self.owner.clone(),
            inner: InboundInner {
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
        // SAFETY: For const reference ptr is guaranteed to be atomicly accessed, always.
        let value = unsafe {
            let ptr = &self.inner.req_id as *const _ as *mut _;
            let atomic = AtomicLongSizeType::from_ptr(ptr);
            atomic.swap(0, std::sync::atomic::Ordering::Acquire)
        };

        Self::to_req_range(value)
    }

    fn to_req_range(i: u64) -> Option<NonZeroRangeType> {
        let [begin, end] = unsafe {
            // SAFETY: Just byte mucking
            transmute::<_, [SizeType; 2]>(i)
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

    /// Retrieves method name
    pub fn method(&self) -> &str {
        let bytes = self.inner.method_bytes();

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
    /// async fn response_examples<R: rpc_it::Config>(b: &mut BytesMut, ib: &mut Inbound< R>) {
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
            .send(WriterDirective::WriteMsg(buf.split().freeze()))
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
            .try_send(WriterDirective::WriteMsg(buf.split().freeze()))
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

    /// This function retrieves a reusable packet from the current inbound while maintaining its
    /// request ID. Consequently, forwarding this request payload directly to another remote could
    /// cause the original client to receive an `unhandled` error response from that remote.
    ///
    /// To properly forward a request to a remote server, with the intention of having the server's
    /// responses directed back to the original client, it is advisable to encode the request
    /// payload into a new packet before sending it to the remote server.
    ///
    /// > Note:
    /// > - Consider exploring more efficient methods for re-routing forwarded requests. For
    /// >   example, a possible approach could involve (1) retrieving the original request ID and
    /// >   (2) mapping it onto the new packet.
    ///
    /// This function returns `None` if the codec in use does not support packet reusability.
    ///
    /// # Warning
    ///
    /// Employing this method is predicated on the assumption that a packet decodable on this end
    /// will also be decodable by the remote side. However, this cannot be guaranteed as the remote
    /// side might not use the same codec. Therefore, it is not advisable to use this method in
    /// scenarios where the remote side is not under your control, due to the potential for
    /// unexpected behavior or security risks.
    pub fn clone_reusable_packet(&self) -> Option<PreparedPacket<R::Codec>> {
        let Ok(hash) = (self.codec().codec_type_hash_ptr() as usize).try_into() else {
            return None;
        };

        Some(PreparedPacket::new_unchecked(
            self.inner.buffer.clone(),
            hash,
        ))
    }
}

impl InboundInner {
    pub(crate) fn method_bytes(&self) -> &[u8] {
        &self.buffer[self.method.range()]
    }
}

impl<R: Config> Drop for Inbound<R> {
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

impl<R: Config> ParseMessage<R::Codec> for Inbound<R> {
    fn codec_payload_pair(&self) -> (&R::Codec, &[u8]) {
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

// ========================================================== Request Forwarding ===|

/// Forwarding request involves complicated steps, and following error types are defined to describe
/// each errors possible during the process.
#[derive(Error, Debug)]
pub enum ForwardRequestError<RemoteCodec: Codec> {
    #[error("This message is not request")]
    InboundNotRequest,

    #[error("Codec of self instance does not support packet reusability")]
    SelfCodecNotReusable,

    #[error("Codec reusability mismatch with target")]
    CodecTypeMismatch,

    #[error("Failed to retrieve request ID from inbound")]
    RequestIdRetrievalFailed,

    #[error("Request with the same ID already registered on wait list")]
    RequestIdDuplicated,

    #[error("Interrupted")]
    Interrupted,

    #[error("Failed to send request")]
    ForwardingFailed(SendMsgError),

    #[error("Failed to receive response from forwarded server")]
    ReceiveFailed(ResponseReceiveError<RemoteCodec>),

    #[error("Sending back the response failed")]
    ResponseFailed(SendResponseError),
}

impl<R: Config> Inbound<R> {
    // TODO: async fn forward_request(self, other: &RequestSender) -> ...
    //
    // - Tries to forward the request to the other remote.
    // - If the request is already responded, it'll return error.
    // - Maybe implemented via request id table mapping ..?
    //
    // IMPL => Forward to remote -> await -> receive response -> send response to source
    //   - seems we need a way to register custom rpc id on mapping table.
    pub async fn forward_request<Other: Config>(
        self,
        target: &RequestSender<Other>,
        interrupt: impl Future<Output = ()>,
    ) -> Result<(), ForwardRequestError<Other::Codec>> {
        todo!()
    }
}
