use std::{
    fmt::Debug,
    future::Future,
    num::NonZeroUsize,
    ops::Range,
    sync::{Arc, Weak},
};

use crate::{
    codec::{self, Codec, DecodeError, Framing, FramingError},
    transport::{AsyncRead, AsyncReadFrame, AsyncWriteFrame},
};

use async_lock::Mutex as AsyncMutex;
use bitflags::bitflags;
use bytes::{Bytes, BytesMut};
use req::RequestContext;
pub use req::ResponseFuture;

/* ---------------------------------------------------------------------------------------------- */
/*                                            CONSTANTS                                           */
/* ---------------------------------------------------------------------------------------------- */

/* ---------------------------------------------------------------------------------------------- */
/*                                          FEATURE FLAGS                                         */
/* ---------------------------------------------------------------------------------------------- */
bitflags! {
    #[derive(Default, Debug, Clone, Copy)]
    pub struct Feature : u32 {
        /// For inbound request messages, if the request object was dropped without sending
        /// response, the background driver will automatically send 'unhandled' response to the
        /// remote end.
        ///
        /// If you don't want any undesired response to be sent, or you're creating a client handle,
        /// which usually does not receive any request, you can disable this feature.
        const AUTO_RESPONSE_UNHANDLED =         1 << 01;

        /// Do not receive any request from the remote end.
        const NO_RECEIVE_REQUEST =              1 << 02;

        /// Do not receive any notification from the remote end.
        const NO_RECEIVE_NOTIFY =               1 << 03;
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                          BACKED TRAIT                                          */
/* ---------------------------------------------------------------------------------------------- */
/// Creates RPC connection from [`crate::transport::AsyncReadFrame`] and
/// [`crate::transport::AsyncWriteFrame`], and [`crate::codec::Codec`].
///
/// For unsupported features(e.g. notify from client), the codec should return
/// [`crate::codec::EncodeError::UnsupportedFeature`] error.
struct ConnectionImpl<C, T, R> {
    codec: Arc<C>,
    write: AsyncMutex<T>,
    reqs: R,
    tx_drive: flume::Sender<InboundDriverDirective>,
    features: Feature,
}

/// Wraps connection implementation with virtual dispatch.
trait Connection: Send + Sync + 'static + Debug {
    fn codec(&self) -> &dyn Codec;
    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame>;
    fn reqs(&self) -> Option<&RequestContext>;
    fn tx_drive(&self) -> &flume::Sender<InboundDriverDirective>;
    fn feature_flag(&self) -> Feature;
}

impl<C, T, R> Connection for ConnectionImpl<C, T, R>
where
    C: Codec,
    T: AsyncWriteFrame,
    R: GetRequestContext,
{
    fn codec(&self) -> &dyn Codec {
        &*self.codec
    }

    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame> {
        &self.write
    }

    fn reqs(&self) -> Option<&RequestContext> {
        self.reqs.get_req_con()
    }

    fn tx_drive(&self) -> &flume::Sender<InboundDriverDirective> {
        &self.tx_drive
    }

    fn feature_flag(&self) -> Feature {
        self.features
    }
}

impl<C, T, R> ConnectionImpl<C, T, R>
where
    C: Codec,
    T: AsyncWriteFrame,
    R: GetRequestContext,
{
    fn with_req(self) -> ConnectionImpl<C, T, RequestContext> {
        ConnectionImpl {
            codec: self.codec,
            write: self.write,
            reqs: RequestContext::default(),
            tx_drive: self.tx_drive,
            features: self.features,
        }
    }

    fn dyn_ref(&self) -> &dyn Connection {
        self
    }
}

/// Trick to get the request context from the generic type, and to cast [`ConnectionBody`] to
/// `dyn` [`Connection`] trait object.
trait GetRequestContext: std::fmt::Debug + Send + Sync + 'static {
    fn get_req_con(&self) -> Option<&RequestContext>;
}

impl GetRequestContext for RequestContext {
    fn get_req_con(&self) -> Option<&RequestContext> {
        Some(self)
    }
}

impl GetRequestContext for () {
    fn get_req_con(&self) -> Option<&RequestContext> {
        None
    }
}

impl<C, T, R: Debug> Debug for ConnectionImpl<C, T, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionBody").field("reqs", &self.reqs).finish()
    }
}

/// Which couldn't be handled within the non-async drop handlers ...
///
/// - A received request object is dropped before response is sent, therefore an 'aborted' message
///   should be sent to the receiver
#[derive(Debug)]
enum InboundDriverDirective {
    /// Defer sending response to the background task.
    DeferredWrite(DeferredWrite),

    /// Manually close the connection.
    Close,
}

#[derive(Debug)]
enum DeferredWrite {
    /// Send error response to the request
    ErrorResponse(msg::RequestInner, codec::PredefinedResponseError),

    /// Send request was deferred. This is used for non-blocking response, etc.
    Raw(bytes::Bytes),
}

/* ---------------------------------------------------------------------------------------------- */
/*                                             HANDLES                                            */
/* ---------------------------------------------------------------------------------------------- */
/// Bidirectional RPC handle.
#[derive(Clone, Debug)]
pub struct Handle(SendHandle, Receiver);

impl std::ops::Deref for Handle {
    type Target = SendHandle;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Handle {
    pub fn receiver(&self) -> &Receiver {
        &self.1
    }

    pub fn into_receiver(self) -> Receiver {
        self.1
    }

    pub fn split(self) -> (SendHandle, Receiver) {
        (self.0, self.1)
    }

    pub fn into_sender(self) -> SendHandle {
        self.0
    }
}

/// Receive-only handle. This is useful when you want to consume all remaining RPC messages when
/// the connection is being closed.
#[derive(Clone, Debug)]
pub struct Receiver(flume::Receiver<msg::RecvMsg>);

impl Receiver {
    pub fn blocking_recv(&self) -> Result<msg::RecvMsg, RecvError> {
        Ok(self.0.recv()?)
    }

    pub async fn recv(&self) -> Result<msg::RecvMsg, RecvError> {
        Ok(self.0.recv_async().await?)
    }

    pub fn try_recv(&self) -> Result<msg::RecvMsg, TryRecvError> {
        self.0.try_recv().map_err(|e| match e {
            flume::TryRecvError::Empty => TryRecvError::Empty,
            flume::TryRecvError::Disconnected => TryRecvError::Disconnected,
        })
    }

    pub fn is_disconnected(&self) -> bool {
        self.0.is_disconnected()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Send-only handle. This holds strong reference to the connection.
#[derive(Clone, Debug)]
pub struct SendHandle(Arc<dyn Connection>);

/// Reused buffer over multiple RPC request/responses
///
/// To minimize the memory allocation during sender payload serialization, reuse this buffer over
/// multiple RPC request/notification.
#[derive(Debug, Clone, Default)]
pub struct WriteBuffer {
    value: Vec<u8>,
}

impl WriteBuffer {
    pub(crate) fn prepare(&mut self) -> &mut Self {
        self.value.clear();
        self
    }
}

impl SendHandle {
    pub async fn request<'a, T: serde::Serialize>(
        &'a self,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        self.request_with_reuse(&mut Default::default(), method, params).await
    }

    pub async fn notify<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        self.notify_with_reuse(&mut Default::default(), method, params).await
    }

    #[doc(hidden)]
    pub async fn request_with_reuse<T: serde::Serialize>(
        &self,
        buf: &mut WriteBuffer,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        let Some(req) = self.0.reqs() else {
            return Err(SendError::RequestDisabled)
        };

        let req_id_hint = req.next_req_id_base();
        let req_id_hash =
            self.0.codec().encode_request(method, req_id_hint, params, &mut buf.prepare().value)?;

        // Registering request always preceded than sending request. If request was not sent due to
        // I/O issue or cancellation, the request will be unregistered on the drop of the
        // `ResponseFuture`.
        req.register_response_catch(req_id_hash);

        let fut = ResponseFuture::new(&*self.0, req_id_hash);
        self.0.__write_raw(&buf.value).await?;

        Ok(fut)
    }

    #[doc(hidden)]
    pub async fn notify_with_reuse<T: serde::Serialize>(
        &self,
        buf: &mut WriteBuffer,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        self.0.codec().encode_notify(method, params, &mut buf.prepare().value)?;
        self.0.__write_raw(&buf.value).await?;
        Ok(())
    }

    /// Send deferred notification. This method is non-blocking, as the message will be sent in the
    /// background task.
    pub fn notify_deferred<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        let mut vec = Vec::new();
        self.0.codec().encode_notify(method, params, &mut vec)?;
        self.0
            .tx_drive()
            .send(InboundDriverDirective::DeferredWrite(DeferredWrite::Raw(vec.into())))
            .map_err(|_| SendError::Disconnected)
    }

    /// Check if the connection is disconnected.
    pub fn is_disconnected(&self) -> bool {
        self.0.tx_drive().is_disconnected()
    }

    /// Closes the connection. If the
    pub fn close(self) -> bool {
        self.0.tx_drive().send(InboundDriverDirective::Close).is_ok()
    }

    /// Is sending request enabled?
    pub fn is_request_enabled(&self) -> bool {
        self.0.reqs().is_some()
    }

    /// Get feature flags
    pub fn get_feature_flags(&self) -> Feature {
        self.0.feature_flag()
    }
}

mod req {
    use std::sync::atomic::AtomicU64;

    use super::{msg, Connection};

    /// RPC request context. Stores request ID and response receiver context.
    #[derive(Debug, Default)]
    pub(super) struct RequestContext {
        /// We may not run out of 64-bit sequential integers ...
        req_id_gen: AtomicU64,
        // TODO: Registered request futures ...
        // TODO: Handle cancellations ...
    }

    impl RequestContext {
        /// Never returns 0.
        pub fn next_req_id_base(&self) -> u64 {
            // Don't let it be zero, even after we publish 2^63 requests on single connection ...
            1 + self.req_id_gen.fetch_add(2, std::sync::atomic::Ordering::Relaxed)
        }

        pub fn register_response_catch(&self, req_id_hash: u64) {
            todo!("")
        }

        /// Invoked on `ResponseFuture::drop` called
        fn unregister_response_catch(&self, req_id_hash: u64) {
            todo!("")
        }

        /// Routes given response to appropriate handler. Returns `Err` if no handler is found.
        pub fn route_response(
            &self,
            req_id_hash: u64,
            response: msg::Response,
        ) -> Result<(), msg::Response> {
            Ok(())
        }
    }

    /// When dropped, the response handler will be unregistered from the queue.
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct ResponseFuture<'a> {
        handle: &'a dyn Connection,
        req_id_hash: u64,
    }

    impl<'a> ResponseFuture<'a> {
        pub(super) fn new(handle: &'a dyn Connection, req_id_hash: u64) -> Self {
            Self { handle, req_id_hash }
        }
    }

    impl<'a> std::future::Future for ResponseFuture<'a> {
        type Output = msg::Response;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            todo!()
        }
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                        ERROR DEFINIITONS                                       */
/* ---------------------------------------------------------------------------------------------- */
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("Encoding outbound message failed: {0}")]
    CodecError(#[from] crate::codec::EncodeError),

    #[error("Error during preparing send: {0}")]
    SendSetupFailed(std::io::Error),

    #[error("Error during sending message to write stream: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Request feature is disabled for this connection")]
    RequestDisabled,

    #[error("Channel is already closed!")]
    Disconnected,
}

#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    #[error("Channel has been closed")]
    Disconnected,
}

#[derive(Debug, thiserror::Error)]
pub enum TryRecvError {
    #[error("Connection has been disconnected")]
    Disconnected,

    #[error("Channel is empty")]
    Empty,
}

impl From<flume::RecvError> for RecvError {
    fn from(value: flume::RecvError) -> Self {
        match value {
            flume::RecvError::Disconnected => Self::Disconnected,
        }
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                             BUILDER                                            */
/* ---------------------------------------------------------------------------------------------- */

// TODO: Request Builder
//  - Create async worker task to handle receive
pub struct Builder<Tw, Tr, C, E> {
    codec: C,
    write: Tw,
    read: Tr,
    ev: E,

    /// Required configurations
    cfg: BuilderOtherConfig,
}

#[derive(Default)]
struct BuilderOtherConfig {
    enable_send_request: bool,
    inbound_channel_cap: Option<NonZeroUsize>,
    feature_flag: Feature,
}

impl Default for Builder<(), (), (), EmptyEventListener> {
    fn default() -> Self {
        Self { codec: (), write: (), read: (), cfg: Default::default(), ev: EmptyEventListener }
    }
}

impl<Tw, Tr, C, E> Builder<Tw, Tr, C, E> {
    /// Specify codec to use
    pub fn with_codec<C1: Codec>(self, codec: C1) -> Builder<Tw, Tr, C1, E> {
        Builder { codec, write: self.write, read: self.read, cfg: self.cfg, ev: self.ev }
    }

    /// Specify write frame to use
    pub fn with_write<Tw2: AsyncWriteFrame>(self, write: Tw2) -> Builder<Tw2, Tr, C, E> {
        Builder { codec: self.codec, write, read: self.read, cfg: self.cfg, ev: self.ev }
    }

    pub fn with_read<Tr2: AsyncReadFrame>(self, read: Tr2) -> Builder<Tw, Tr2, C, E> {
        Builder { codec: self.codec, write: self.write, read, cfg: self.cfg, ev: self.ev }
    }

    pub fn with_event_listener<E2: InboundEventSubscriber>(self, ev: E2) -> Builder<Tw, Tr, C, E2> {
        Builder { codec: self.codec, write: self.write, read: self.read, cfg: self.cfg, ev }
    }

    /// Set the read frame with stream reader and framing decoder.
    ///
    /// # Parameters
    ///
    /// - `fallback_buffer_size`: When the framing decoder does not provide the next buffer size,
    ///   this value is used to pre-allocate the buffer for the next [`AsyncReadFrame::poll_read`]
    ///   call.
    pub fn with_read_stream<Tr2: AsyncRead, F: Framing>(
        self,
        read: Tr2,
        framing: F,
    ) -> Builder<Tw, impl AsyncReadFrame, C, E> {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct FramingReader<T, F> {
            reader: T,
            framing: F,
        }

        impl<T, F> AsyncReadFrame for FramingReader<T, F>
        where
            T: AsyncRead,
            F: Framing,
        {
            fn poll_read(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut BytesMut,
            ) -> Poll<Result<Bytes, std::io::Error>> {
                let this = self.get_mut();

                loop {
                    // Reading as many as possible is almostly desireable.
                    let size_hint = this.framing.next_buffer_size();
                    let mut buf_read = buf.split_to(0);

                    let Poll::Ready(_) = Pin::new(&mut this.reader).poll_read(cx, &mut buf_read, size_hint)? else {
                        break Poll::Pending;
                    };

                    buf.unsplit(buf_read);

                    // Try decode the buffer.
                    match this.framing.advance(&buf[..]) {
                        Ok(Some(x)) => {
                            debug_assert!(x.valid_data_end <= x.next_frame_start);
                            debug_assert!(x.next_frame_start <= buf.len());

                            let mut frame = buf.split_to(x.next_frame_start);
                            break Poll::Ready(Ok(frame.split_off(x.valid_data_end).into()));
                        }

                        // Just poll once more, until it retunrs 'Pending' ...
                        Ok(None) => continue,

                        Err(FramingError::Broken(why)) => {
                            return Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                why,
                            )))
                        }

                        Err(FramingError::Recoverable(num_discard_bytes)) => {
                            debug_assert!(num_discard_bytes <= buf.len());

                            let _ = buf.split_to(num_discard_bytes);
                            continue;
                        }
                    }
                }
            }
        }

        Builder {
            codec: self.codec,
            write: self.write,
            read: FramingReader { reader: read, framing },
            cfg: self.cfg,
            ev: self.ev,
        }
    }

    /// Enable or disable send request feature. Default is disabled.
    pub fn with_send_request(mut self, enabled: bool) -> Self {
        self.cfg.enable_send_request = enabled;
        self
    }

    /// Setting zero will create unbounded channel. Default is unbounded.
    pub fn with_inbound_channel_capacity(mut self, capacity: usize) -> Self {
        self.cfg.inbound_channel_cap = capacity.try_into().ok();
        self
    }

    pub fn with_feature(mut self, feature: Feature) -> Self {
        self.cfg.feature_flag |= feature;
        self
    }

    pub fn without_feature(mut self, feature: Feature) -> Self {
        self.cfg.feature_flag &= !feature;
        self
    }
}

impl<Tw, Tr, C, E> Builder<Tw, Tr, C, E>
where
    Tw: AsyncWriteFrame,
    Tr: AsyncReadFrame,
    C: Codec,
    E: InboundEventSubscriber,
{
    pub fn build(self) -> Result<(Handle, impl Future), std::io::Error> {
        let mut fut_with_request = None;
        let mut fut_without_request = None;

        let (tx_inb_drv, rx_inb_drv) = flume::unbounded();
        let (tx_in_msg, rx_in_msg) =
            if let Some(chan_cap) = self.cfg.inbound_channel_cap.map(|x| x.get()) {
                flume::bounded(chan_cap)
            } else {
                flume::unbounded()
            };

        let conn: Arc<dyn Connection>;
        let this = ConnectionImpl {
            codec: self.codec.into(),
            write: AsyncMutex::new(self.write),
            reqs: (),
            tx_drive: tx_inb_drv,
            features: self.cfg.feature_flag,
        };

        if self.cfg.enable_send_request {
            let this = Arc::new(this.with_req());
            fut_with_request =
                Some(<ConnectionImpl<_, _, RequestContext>>::inbound_event_handler(DriverBody {
                    w_this: Arc::downgrade(&this),
                    read: self.read,
                    ev_subs: self.ev,
                    rx_drive: rx_inb_drv,
                    tx_msg: tx_in_msg,
                }));

            conn = this;
        } else {
            let this = Arc::new(this);
            fut_without_request =
                Some(<ConnectionImpl<_, _, ()>>::inbound_event_handler(DriverBody {
                    w_this: Arc::downgrade(&this),
                    read: self.read,
                    ev_subs: self.ev,
                    rx_drive: rx_inb_drv,
                    tx_msg: tx_in_msg,
                }));

            conn = this;
        }

        Ok((Handle(SendHandle(conn), Receiver(rx_in_msg)), async move {
            if let Some(fut) = fut_with_request {
                fut.await;
            } else if let Some(fut) = fut_without_request {
                fut.await;
            } else {
                unreachable!()
            }
        }))
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                      EVENT LISTENER TRAIT                                      */
/* ---------------------------------------------------------------------------------------------- */

#[derive(Debug, thiserror::Error)]
pub enum InboundError {
    #[error("Response packet is received, but we haven't enabled request feature!")]
    RedundantResponse(msg::Response),

    #[error("Response packet is not routed.")]
    ExpiredResponse(msg::Response),

    #[error("Failed to decode inbound type: {} bytes, error was = {0} ", .1.len())]
    InboundDecodeError(DecodeError, bytes::Bytes),

    #[error("Disabled notification/request is received")]
    DisabledInbound(msg::RecvMsg),
}

/// This trait is to notify the event from the connection.
pub trait InboundEventSubscriber: Send + Sync + 'static {
    /// Called when the inbound stream is closed. If channel is closed
    ///
    /// # Arguments
    ///
    /// - `closed_by_us`: Are we closing? Otherwise, the read stream is closed by the remote.
    /// - `result`:
    ///     - Closing result of the read stream. Usually, if `closed_by_us` is true, this is
    ///       `Ok(())`, otherwise, may contain error related to network stream disconnection.
    fn on_close(&self, closed_by_us: bool, result: std::io::Result<()>) {
        let _ = (closed_by_us, result);
    }

    /// When an errnous response is received.
    ///
    /// -
    fn on_inbound_error(&self, error: InboundError) {
        let _ = (error,);
    }
}

/// Placeholder implementation of event listener.
struct EmptyEventListener;

impl InboundEventSubscriber for EmptyEventListener {}

/* ---------------------------------------------------------------------------------------------- */
/*                                            MESSAGES                                            */
/* ---------------------------------------------------------------------------------------------- */
pub trait Message {
    fn payload(&self) -> &[u8];
    fn raw(&self) -> &[u8];
    fn raw_bytes(&self) -> Bytes;
    fn codec(&self) -> &dyn Codec;

    /// Parses payload as speicfied type.
    fn parse<'a, T: serde::de::Deserialize<'a>>(&'a self) -> Result<T, DecodeError> {
        let mut dst = None;
        self.parse_in_place(&mut dst)?;
        Ok(dst.unwrap())
    }

    /// Parses payload as speicfied type, in-place. It is marked as hidden to follow origianl method
    /// [`serde::Deserialize::deserialize_in_place`]'s visibility convention, which is '*almost
    /// never what newbies are looking for*'.
    #[doc(hidden)]
    fn parse_in_place<'a, T: serde::de::Deserialize<'a>>(
        &'a self,
        dst: &mut Option<T>,
    ) -> Result<(), DecodeError> {
        self.codec().decode_payload(self.payload(), &mut |de| {
            if let Some(dst) = dst.as_mut() {
                T::deserialize_in_place(de, dst)
            } else {
                *dst = Some(T::deserialize(de)?);
                Ok(())
            }
        })
    }
}

pub trait MessageReqId: Message {
    fn req_id(&self) -> &[u8];
}

pub trait MessageMethodName: Message {
    fn method_raw(&self) -> &[u8];
    fn method(&self) -> Option<&str> {
        std::str::from_utf8(self.method_raw()).ok()
    }
}

/// Common content of inbound message
#[derive(Debug)]
struct InboundBody {
    buffer: Bytes,
    payload: Range<usize>,
    codec: Arc<dyn Codec>,
}

pub mod msg {
    use std::{ops::Range, sync::Arc};

    use enum_as_inner::EnumAsInner;

    use crate::{
        codec::{DecodeError, PredefinedResponseError},
        rpc::MessageReqId,
    };

    use super::{Connection, Message};

    macro_rules! impl_message {
        ($t:ty) => {
            impl super::Message for $t {
                fn payload(&self) -> &[u8] {
                    &self.h.buffer[self.h.payload.clone()]
                }

                fn raw(&self) -> &[u8] {
                    &self.h.buffer
                }

                fn raw_bytes(&self) -> bytes::Bytes {
                    self.h.buffer.clone()
                }

                fn codec(&self) -> &dyn super::Codec {
                    &*self.h.codec
                }
            }
        };
    }

    macro_rules! impl_method_name {
        ($t:ty) => {
            impl super::MessageMethodName for $t {
                fn method_raw(&self) -> &[u8] {
                    &self.h.buffer[self.method.clone()]
                }
            }
        };
    }

    macro_rules! impl_req_id {
        ($t:ty) => {
            impl super::MessageReqId for $t {
                fn req_id(&self) -> &[u8] {
                    &self.h.buffer[self.req_id.clone()]
                }
            }
        };
    }

    /* ------------------------------------- Request Logics ------------------------------------- */
    #[derive(Debug)]
    pub struct RequestInner {
        pub(super) h: super::InboundBody,
        pub(super) method: Range<usize>,
        pub(super) req_id: Range<usize>,
    }

    impl_message!(RequestInner);
    impl_method_name!(RequestInner);
    impl_req_id!(RequestInner);

    #[derive(Debug)]
    pub struct Request {
        pub(super) body: Option<(RequestInner, Arc<dyn Connection>)>,
    }

    impl std::ops::Deref for Request {
        type Target = RequestInner;

        fn deref(&self) -> &Self::Target {
            &self.body.as_ref().unwrap().0
        }
    }

    impl Request {
        // TODO: send response

        /// Response with given value. If the value is `Err`, the response will be sent as error
        #[doc(hidden)]
        pub async fn response_with_reuse<T: serde::Serialize>(
            mut self,
            buf: &mut super::WriteBuffer,
            value: Result<&T, &T>,
        ) -> Result<(), super::SendError> {
            let (inner, conn) = self.body.take().unwrap();
            buf.prepare();

            let encode_as_error = value.is_err();
            let value = value.unwrap_or_else(|x| x);
            self.codec().encode_response(inner.req_id(), encode_as_error, value, &mut buf.value)?;

            conn.__write_raw(&buf.value).await?;
            Ok(())
        }

        /// Explicitly abort the request. This is useful when you want to cancel the request
        pub async fn abort(self) -> Result<(), super::SendError> {
            self.error_predefined(PredefinedResponseError::Aborted).await
        }

        /// Response with 'parse failed' predefined error type.
        pub async fn error_parse_failed<T>(self) -> Result<(), super::SendError> {
            let err = PredefinedResponseError::ParseFailed(std::any::type_name::<T>());
            self.error_predefined(err).await
        }

        /// Response 'internal' error with given error code.
        pub async fn error_internal(
            self,
            errc: i32,
            detail: Option<String>,
        ) -> Result<(), super::SendError> {
            let err = if let Some(detail) = detail {
                PredefinedResponseError::InternalDetailed(errc, detail)
            } else {
                PredefinedResponseError::Internal(errc)
            };

            self.error_predefined(err).await
        }

        async fn error_predefined(
            mut self,
            err: super::codec::PredefinedResponseError,
        ) -> Result<(), super::SendError> {
            let (inner, conn) = self.body.take().unwrap();
            conn.__send_err_predef(&mut Default::default(), &inner, &err).await
        }
    }

    impl Drop for Request {
        fn drop(&mut self) {
            if let Some((inner, conn)) = self.body.take() {
                conn.tx_drive()
                    .send(super::InboundDriverDirective::DeferredWrite(
                        super::DeferredWrite::ErrorResponse(
                            inner,
                            PredefinedResponseError::Unhandled,
                        ),
                    ))
                    .ok();
            }
        }
    }

    /* -------------------------------------- Notify Logics ------------------------------------- */
    #[derive(Debug)]
    pub struct Notify {
        pub(super) h: super::InboundBody,
        pub(super) method: Range<usize>,
    }

    impl_message!(Notify);
    impl_method_name!(Notify);

    #[derive(Debug, EnumAsInner)]
    pub enum RecvMsg {
        Request(Request),
        Notify(Notify),
    }

    impl super::Message for RecvMsg {
        fn payload(&self) -> &[u8] {
            match self {
                Self::Request(x) => x.payload(),
                Self::Notify(x) => x.payload(),
            }
        }

        fn raw(&self) -> &[u8] {
            match self {
                Self::Request(x) => x.raw(),
                Self::Notify(x) => x.raw(),
            }
        }

        fn raw_bytes(&self) -> bytes::Bytes {
            match self {
                Self::Request(x) => x.raw_bytes(),
                Self::Notify(x) => x.raw_bytes(),
            }
        }

        fn codec(&self) -> &dyn crate::codec::Codec {
            match self {
                Self::Request(x) => x.codec(),
                Self::Notify(x) => x.codec(),
            }
        }
    }

    impl super::MessageMethodName for RecvMsg {
        fn method_raw(&self) -> &[u8] {
            match self {
                Self::Request(x) => x.method_raw(),
                Self::Notify(x) => x.method_raw(),
            }
        }
    }

    /* ------------------------------------- Response Logics ------------------------------------ */
    #[derive(Debug)]
    pub struct Response {
        pub(super) h: super::InboundBody,
        pub(super) req_id: Range<usize>,

        /// Should we interpret the payload as error object?
        pub(super) is_error: bool,
    }

    impl_message!(Response);
    impl_req_id!(Response);

    impl Response {
        pub fn is_error(&self) -> bool {
            self.is_error
        }

        pub fn result<'a, T: serde::Deserialize<'a>, E: serde::Deserialize<'a>>(
            &'a self,
        ) -> Result<Result<T, E>, DecodeError> {
            if self.is_error {
                self.parse().map(Err)
            } else {
                self.parse().map(Ok)
            }
        }
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                      DRIVER IMPLEMENTATION                                     */
/* ---------------------------------------------------------------------------------------------- */

struct DriverBody<C, T, E, R, Tr> {
    w_this: Weak<ConnectionImpl<C, T, R>>,
    read: Tr,
    ev_subs: E,
    rx_drive: flume::Receiver<InboundDriverDirective>,
    tx_msg: flume::Sender<msg::RecvMsg>,
}

mod inner {
    //! Internal driver context. It receives the request from the connection, and dispatches it to
    //! the handler.

    use capture_it::capture;
    use futures_util::{future::FusedFuture, FutureExt};
    use std::{future::poll_fn, pin::Pin, sync::Arc};

    use crate::{
        codec::{self, Codec, InboundFrameType},
        rpc::{DeferredWrite, SendError},
        transport::{AsyncReadFrame, AsyncWriteFrame},
    };

    use super::{
        msg, ConnectionImpl, DriverBody, Feature, GetRequestContext, InboundBody,
        InboundDriverDirective, InboundError, InboundEventSubscriber, MessageReqId, RequestContext,
    };

    /// Request-acceting version of connection driver
    impl<C, T> ConnectionImpl<C, T, RequestContext>
    where
        C: Codec,
        T: AsyncWriteFrame,
    {
        pub(crate) async fn inbound_event_handler<Tr: AsyncReadFrame, E: InboundEventSubscriber>(
            body: DriverBody<C, T, E, RequestContext, Tr>,
        ) {
            body.execute(|this, ev, param| {
                if let Err(msg) = this.reqs.route_response(param.req_id_hash, param.response) {
                    ev.on_inbound_error(InboundError::ExpiredResponse(msg));
                }
            })
            .await;
        }
    }

    /// Non-request-accepting version of connection driver
    impl<C, T> ConnectionImpl<C, T, ()>
    where
        C: Codec,
        T: AsyncWriteFrame,
    {
        pub(crate) async fn inbound_event_handler<Tr: AsyncReadFrame, E: InboundEventSubscriber>(
            body: DriverBody<C, T, E, (), Tr>,
        ) {
            // Naively call the implementation with empty handler. Just discards all responses
            body.execute(|_, ev, param| {
                ev.on_inbound_error(InboundError::RedundantResponse(param.response))
            })
            .await
        }
    }

    impl<C, T, E, R, Tr> DriverBody<C, T, E, R, Tr>
    where
        C: Codec,
        T: AsyncWriteFrame,
        E: InboundEventSubscriber,
        R: GetRequestContext,
        Tr: AsyncReadFrame,
    {
        async fn execute(
            self,
            fn_handle_response: impl Fn(&Arc<ConnectionImpl<C, T, R>>, &mut E, HandleResponseParam),
        ) {
            let DriverBody { w_this, mut read, mut ev_subs, rx_drive: rx_msg, tx_msg } = self;
            let mut rx_buf = bytes::BytesMut::new();

            use futures_util::future::Fuse;
            let mut fut_drive_msg = Fuse::terminated();
            let mut fut_read = Fuse::terminated();
            let mut close_from_remote = false;

            /* ----------------------------- Background Sender Task ----------------------------- */
            // Tasks that perform deferred write operations in the background. It handles messages
            // sent from non-async non-blocking context, such as 'unhandled' response pushed inside
            // `Drop` handler.
            let (tx_bg_sender, rx_bg_sender) = flume::unbounded();
            let fut_bg_sender = capture!([w_this], async move {
                let mut pool = super::WriteBuffer::default();
                while let Ok(msg) = rx_bg_sender.recv_async().await {
                    match msg {
                        DeferredWrite::ErrorResponse(req, err) => {
                            let Some(this) = w_this.upgrade() else { break };
                            let err = this.dyn_ref().__send_err_predef(&mut pool, &req, &err).await;

                            // Ignore all other error types, as this message is triggered
                            // crate-internally on very limited situations
                            let Err(SendError::IoError(e)) = err else { continue };
                            return Err(e);
                        }
                        DeferredWrite::Raw(msg) => {
                            let Some(this) = w_this.upgrade() else { break };
                            this.dyn_ref().__write_raw(&msg).await?;
                        }
                    }
                }

                Ok::<(), std::io::Error>(())
            });
            let fut_bg_sender = fut_bg_sender.fuse();
            let mut fut_bg_sender = std::pin::pin!(fut_bg_sender);

            /* ------------------------------------ App Loop ------------------------------------ */
            loop {
                if fut_drive_msg.is_terminated() {
                    fut_drive_msg = rx_msg.recv_async().fuse();
                }

                if fut_read.is_terminated() {
                    fut_read = poll_fn(|cx| Pin::new(&mut read).poll_read(cx, &mut rx_buf)).fuse();
                }

                futures_util::select! {
                    msg = fut_drive_msg => {
                        match msg {
                            Ok(InboundDriverDirective::DeferredWrite(msg)) => {
                                // This may not fail.
                                tx_bg_sender.send_async(msg).await.ok();
                            }

                            Err(_) | Ok(InboundDriverDirective::Close) => {
                                // Connection disposed by 'us' (by dropping RPC handle). i.e.
                                // `close_from_remote = false`
                                break;
                            }
                        }
                    }

                    inbound = fut_read => {
                        let Some(this) = w_this.upgrade() else { break };

                        match inbound {
                            Ok(bytes) => {
                                Self::on_read(
                                    &this,
                                    &mut ev_subs,
                                    &fn_handle_response,
                                    bytes,
                                    &tx_msg
                                )
                                .await;
                            }
                            Err(e) => {
                                close_from_remote = true;
                                ev_subs.on_close(false, Err(e));
                            }
                        }
                    }

                    result = fut_bg_sender => {
                        if let Err(err) = result {
                            close_from_remote = true;
                            ev_subs.on_close(true, Err(err));
                        }

                        break;
                    }
                }
            }

            // If we're exitting with alive handle, manually close the write stream.
            if let Some(x) = w_this.upgrade() {
                x.dyn_ref().__close().await.ok();
            }

            // Just try to close the channel
            if !close_from_remote {
                let r = poll_fn(|cx| Pin::new(&mut read).poll_close(cx)).await;
                ev_subs.on_close(true, r); // We're closing this
            }
        }

        async fn on_read(
            this: &Arc<ConnectionImpl<C, T, R>>,
            ev_subs: &mut E,
            fn_handle_response: &impl Fn(&Arc<ConnectionImpl<C, T, R>>, &mut E, HandleResponseParam),
            frame: bytes::Bytes,
            tx_msg: &flume::Sender<msg::RecvMsg>,
        ) {
            let parsed = this.codec.decode_inbound(&frame);
            let (header, payload_span) = match parsed {
                Ok(x) => x,
                Err(e) => {
                    ev_subs.on_inbound_error(InboundError::InboundDecodeError(e, frame));
                    return;
                }
            };

            let h = InboundBody { buffer: frame, payload: payload_span, codec: this.codec.clone() };
            match header {
                InboundFrameType::Notify { .. } | InboundFrameType::Request { .. } => {
                    let (msg, disabled) = match header {
                        InboundFrameType::Notify { method } => (
                            msg::RecvMsg::Notify(msg::Notify { h, method }),
                            this.features.contains(Feature::NO_RECEIVE_NOTIFY),
                        ),
                        InboundFrameType::Request { method, req_id } => (
                            msg::RecvMsg::Request(msg::Request {
                                body: Some((msg::RequestInner { h, method, req_id }, this.clone())),
                            }),
                            this.features.contains(Feature::NO_RECEIVE_REQUEST),
                        ),
                        _ => unreachable!(),
                    };

                    if disabled {
                        ev_subs.on_inbound_error(InboundError::DisabledInbound(msg));
                    } else {
                        tx_msg.send_async(msg).await.ok();
                    }
                }

                InboundFrameType::Response { req_id, req_id_hash, is_error } => {
                    fn_handle_response(
                        this,
                        ev_subs,
                        HandleResponseParam {
                            response: msg::Response { h, req_id, is_error },
                            req_id_hash,
                        },
                    );
                }
            }
        }
    }

    impl dyn super::Connection {
        pub(crate) async fn __send_err_predef(
            &self,
            buf: &mut super::WriteBuffer,
            recv: &msg::RequestInner,
            error: &codec::PredefinedResponseError,
        ) -> Result<(), super::SendError> {
            self.codec().encode_response_predefined(
                recv.req_id(),
                error,
                &mut buf.prepare().value,
            )?;

            self.__write_raw(&buf.prepare().value).await?;
            Ok(())
        }

        pub(crate) async fn __write_raw(&self, buf: &[u8]) -> std::io::Result<()> {
            let mut write = self.write().lock().await;
            poll_fn(|cx| Pin::new(&mut *write).poll_pre_write(cx)).await?;
            poll_fn(|cx| Pin::new(&mut *write).poll_write(cx, buf)).await?;
            Ok(())
        }

        pub(crate) async fn __flush(&self) -> std::io::Result<()> {
            todo!()
        }

        pub(crate) async fn __close(&self) -> std::io::Result<()> {
            let mut write = self.write().lock().await;
            poll_fn(|cx| Pin::new(&mut *write).poll_close(cx)).await
        }
    }

    struct HandleResponseParam {
        response: msg::Response,
        req_id_hash: u64,
    }
}
