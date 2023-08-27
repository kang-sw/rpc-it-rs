//! RPC connection implementation
//!
//! # Cautions
//!
//! - All `write` operation are not cancellation-safe in async context. Once you abort the async
//!   write task such as `request`, `notify`, `response*` series, the write stream may remain in
//!   corrupted state, which may invalidate any subsequent write operation.
//!

use std::{
    any::Any,
    fmt::Debug,
    num::NonZeroUsize,
    ops::Range,
    sync::{Arc, Weak},
};

use crate::{
    codec::{self, Codec, DecodeError, Framing, FramingError},
    transport::{AsyncFrameRead, AsyncFrameWrite},
};

use async_lock::Mutex as AsyncMutex;
use bitflags::bitflags;
use bytes::{Bytes, BytesMut};
use enum_as_inner::EnumAsInner;
use futures_util::AsyncRead;
pub use req::RequestContext;
pub use req::{OwnedResponseFuture, ResponseFuture};

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
        const ENABLE_AUTO_RESPONSE =            1 << 1;

        /// Do not receive any request from the remote end.
        const NO_RECEIVE_REQUEST =              1 << 2;

        /// Do not receive any notification from the remote end.
        const NO_RECEIVE_NOTIFY =               1 << 3;
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
struct ConnectionImpl<C, T, R, U> {
    codec: Arc<C>,
    write: AsyncMutex<T>,
    reqs: R,
    tx_drive: flume::Sender<InboundDriverDirective>,
    features: Feature,
    user_data: U,
    _unpin: std::marker::PhantomPinned,
}

/// Wraps connection implementation with virtual dispatch.
trait Connection: Send + Sync + 'static + Debug {
    fn codec(&self) -> &dyn Codec;
    fn write(&self) -> &AsyncMutex<dyn AsyncFrameWrite>;
    fn reqs(&self) -> Option<&RequestContext>;
    fn tx_drive(&self) -> &flume::Sender<InboundDriverDirective>;
    fn feature_flag(&self) -> Feature;
    fn user_data(&self) -> &dyn UserData;
}

impl<C, T, R, U> Connection for ConnectionImpl<C, T, R, U>
where
    C: Codec,
    T: AsyncFrameWrite,
    R: GetRequestContext,
    U: UserData,
{
    fn codec(&self) -> &dyn Codec {
        &*self.codec
    }

    fn write(&self) -> &AsyncMutex<dyn AsyncFrameWrite> {
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

    fn user_data(&self) -> &dyn UserData {
        &self.user_data
    }
}

impl<C, T, R, U> ConnectionImpl<C, T, R, U>
where
    C: Codec,
    T: AsyncFrameWrite,
    R: GetRequestContext,
    U: UserData,
{
    fn dyn_ref(&self) -> &dyn Connection {
        self
    }
}

/// Trick to get the request context from the generic type, and to cast [`ConnectionBody`] to
/// `dyn` [`Connection`] trait object.
pub trait GetRequestContext: std::fmt::Debug + Send + Sync + 'static {
    fn get_req_con(&self) -> Option<&RequestContext>;
}

impl GetRequestContext for Arc<RequestContext> {
    fn get_req_con(&self) -> Option<&RequestContext> {
        Some(self)
    }
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

impl<C, T, R: Debug, U> Debug for ConnectionImpl<C, T, R, U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionBody").field("reqs", &self.reqs).finish()
    }
}

/* --------------------------------------- User Data Trait -------------------------------------- */
pub trait UserData: Any + Send + Sync + 'static {
    fn as_any(&self) -> &(dyn Any + Send + Sync + 'static);
}

impl<T> UserData for T
where
    T: Any + Send + Sync + 'static,
{
    fn as_any(&self) -> &(dyn Any + Send + Sync + 'static) {
        self
    }
}

impl AsRef<dyn Any + Send + Sync + 'static> for dyn UserData {
    fn as_ref(&self) -> &(dyn Any + Send + Sync + 'static) {
        self.as_any()
    }
}

#[derive(Clone)]
pub struct OwnedUserData(Arc<dyn Connection>);

impl OwnedUserData {
    pub fn raw(&self) -> &dyn UserData {
        self.0.user_data()
    }
}

impl std::ops::Deref for OwnedUserData {
    type Target = dyn Any;

    fn deref(&self) -> &Self::Target {
        self.0.user_data().as_any()
    }
}

/* -------------------------------------- Driver Directives ------------------------------------- */

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
    Raw(bytes::BytesMut),

    /// Send flush request from background
    Flush,
}

/* ---------------------------------------------------------------------------------------------- */
/*                                             HANDLES                                            */
/* ---------------------------------------------------------------------------------------------- */
/// Bidirectional RPC handle. It can serve as both client and server.
#[derive(Clone, Debug)]
pub struct Transceiver(Sender, flume::Receiver<msg::RecvMsg>);

impl std::ops::Deref for Transceiver {
    type Target = Sender;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Transceiver {
    pub fn into_sender(self) -> Sender {
        self.0
    }

    pub fn clone_sender(&self) -> Sender {
        self.0.clone()
    }

    pub fn blocking_recv(&self) -> Result<msg::RecvMsg, RecvError> {
        Ok(self.1.recv()?)
    }

    pub async fn recv(&self) -> Result<msg::RecvMsg, RecvError> {
        Ok(self.1.recv_async().await?)
    }

    pub fn try_recv(&self) -> Result<msg::RecvMsg, TryRecvError> {
        self.1.try_recv().map_err(|e| match e {
            flume::TryRecvError::Empty => TryRecvError::Empty,
            flume::TryRecvError::Disconnected => TryRecvError::Disconnected,
        })
    }
}

impl From<Transceiver> for Sender {
    fn from(value: Transceiver) -> Self {
        value.0
    }
}

/// Send-only handle. This holds strong reference to the connection.
#[derive(Clone, Debug)]
pub struct Sender(Arc<dyn Connection>);

/// Reused buffer over multiple RPC request/responses
///
/// To minimize the memory allocation during sender payload serialization, reuse this buffer over
/// multiple RPC request/notification.
#[derive(Debug, Clone, Default)]
pub struct WriteBuffer {
    value: BytesMut,
}

impl WriteBuffer {
    pub(crate) fn prepare(&mut self) {
        self.value.clear();
    }

    pub fn reserve(&mut self, size: usize) {
        self.value.clear();
        self.value.reserve(size);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("Failed to send request: {0}")]
    SendFailed(#[from] SendError),

    #[error("Failed to receive response: {0}")]
    FlushFailed(#[from] std::io::Error),

    #[error("Failed to receive response: {0}")]
    RecvFailed(#[from] RecvError),

    #[error("Remote returned invalid return type: {0}")]
    ParseFailed(DecodeError, msg::Response),

    #[error("Remote returned error response")]
    ErrorResponse(msg::Response),
}

#[derive(Debug, EnumAsInner, thiserror::Error)]
pub enum TypedCallError<E> {
    #[error("Failed to send request: {0}")]
    SendFailed(#[from] super::SendError),

    #[error("Failed to receive response: {0}")]
    FlushFailed(#[from] std::io::Error),

    #[error("Failed to receive response: {0}")]
    RecvFailed(#[from] super::RecvError),

    #[error("Remote returned error response")]
    Response(#[from] super::ResponseError<E>),
}

impl Sender {
    /// A shortcut for request, flush, and receive response.
    pub async fn call<R: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<R, CallError> {
        let response = self.request(method, params).await?;

        self.0.__flush().await?;

        let msg = response.await?;

        if msg.is_error {
            return Err(CallError::ErrorResponse(msg));
        }

        match msg.parse() {
            Ok(value) => Ok(value),
            Err(err) => Err(CallError::ParseFailed(err, msg)),
        }
    }

    /// A shortcut for strictly typed request
    pub async fn call_with_err<R: serde::de::DeserializeOwned, E: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<R, TypedCallError<E>> {
        let response = self.request(method, params).await?;

        self.0.__flush().await?;

        let msg = response.await?;
        Ok(msg.result()?)
    }

    /// Send request, and create response future which will be resolved when the response is
    /// received.
    pub async fn request<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        self.request_with_reuse(&mut Default::default(), method, params).await
    }

    /// Send notification message.
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
        let fut = self.prepare_request(buf, method, params)?;
        self.0.__write_raw(&mut buf.value).await?;

        Ok(fut)
    }

    #[doc(hidden)]
    pub async fn notify_with_reuse<T: serde::Serialize>(
        &self,
        buf: &mut WriteBuffer,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        buf.prepare();

        self.0.codec().encode_notify(method, params, &mut buf.value)?;
        self.0.__write_raw(&mut buf.value).await?;

        Ok(())
    }

    /// Sends a request and returns a future that will be resolved when the response is received.
    ///
    /// This method is non-blocking, as the message writing will be deferred to the background
    pub fn request_deferred<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        self.request_deferred_with_reuse(&mut Default::default(), method, params)
    }

    /// Send deferred notification. This method is non-blocking, as the message writing will be
    /// deferred to the background driver worker.
    pub fn notify_deferred<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        self.notify_deferred_with_reuse(&mut Default::default(), method, params)
    }

    /// Sends a request and returns a future that will be resolved when the response is received.
    ///
    /// This method is non-blocking, as the message writing will be deferred to the background
    #[doc(hidden)]
    pub fn request_deferred_with_reuse<T: serde::Serialize>(
        &self,
        buffer: &mut WriteBuffer,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        buffer.prepare();
        let fut = self.prepare_request(buffer, method, params);

        self.0
            .tx_drive()
            .send(InboundDriverDirective::DeferredWrite(DeferredWrite::Raw(buffer.value.split())))
            .map_err(|_| SendError::Disconnected)?;

        fut
    }

    /// Send deferred notification. This method is non-blocking, as the message writing will be
    /// deferred to the background driver worker.
    #[doc(hidden)]
    pub fn notify_deferred_with_reuse<T: serde::Serialize>(
        &self,
        buffer: &mut WriteBuffer,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        buffer.prepare();
        self.0.codec().encode_notify(method, params, &mut buffer.value)?;

        self.0
            .tx_drive()
            .send(InboundDriverDirective::DeferredWrite(DeferredWrite::Raw(buffer.value.split())))
            .map_err(|_| SendError::Disconnected)
    }

    fn prepare_request<T: serde::Serialize>(
        &self,
        buf: &mut WriteBuffer,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        let Some(req) = self.0.reqs() else { return Err(SendError::RequestDisabled) };

        buf.prepare();
        let req_id_hint = req.next_req_id_base();
        let req_id_hash =
            self.0.codec().encode_request(method, req_id_hint, params, &mut buf.value)?;

        // Registering request always preceded than sending request. If request was not sent due to
        // I/O issue or cancellation, the request will be unregistered on the drop of the
        // `ResponseFuture`.
        let slot_id = req.register_req(req_id_hash);

        Ok(ResponseFuture::new(&self.0, slot_id))
    }

    /// Check if the connection is disconnected.
    pub fn is_disconnected(&self) -> bool {
        self.0.tx_drive().is_disconnected()
    }

    /// Closes the connection. If it's already closed, it'll return false.
    ///
    /// # Caution
    ///
    /// If multiple threads calls this method at the same time, more than one thread may return
    /// true, as the close operation is lazy.
    pub fn close(self) -> bool {
        self.0.tx_drive().send(InboundDriverDirective::Close).is_ok()
    }

    /// Flush underlying write stream.
    pub async fn flush(&self) -> std::io::Result<()> {
        self.0.__flush().await
    }

    /// Perform flush from background task.
    pub fn flush_deferred(&self) -> bool {
        self.0.tx_drive().send(InboundDriverDirective::DeferredWrite(DeferredWrite::Flush)).is_ok()
    }

    /// Is sending request enabled?
    pub fn is_request_enabled(&self) -> bool {
        self.0.reqs().is_some()
    }

    /// Get feature flags
    pub fn get_feature_flags(&self) -> Feature {
        self.0.feature_flag()
    }

    /// Get UserData
    pub fn user_data<T: UserData>(&self) -> Option<&T> {
        self.0.user_data().as_any().downcast_ref()
    }
}

mod req {
    use std::{
        borrow::Cow,
        mem::replace,
        num::NonZeroU64,
        sync::{atomic::AtomicU64, Arc},
        task::Poll,
    };

    use dashmap::DashMap;
    use futures_util::{task::AtomicWaker, FutureExt};
    use parking_lot::Mutex;

    use crate::RecvError;

    use super::{msg, Connection};

    /// RPC request context. Stores request ID and response receiver context.
    #[derive(Debug, Default)]
    pub struct RequestContext {
        /// We may not run out of 64-bit sequential integers ...
        req_id_gen: AtomicU64,
        waiters: DashMap<NonZeroU64, RequestSlot>,
    }

    #[derive(Clone, Debug)]
    pub(super) struct RequestSlotId(NonZeroU64);

    #[derive(Debug)]
    struct RequestSlot {
        waker: AtomicWaker,
        value: Mutex<Option<msg::Response>>,
    }

    impl RequestContext {
        /// Never returns 0.
        pub(super) fn next_req_id_base(&self) -> NonZeroU64 {
            // SAFETY: 1 + 2 * N is always odd, and non-zero on wrapping condition.
            // > Additionally, to see it wraps, we need to send 2^63 requests ...
            unsafe {
                NonZeroU64::new_unchecked(
                    1 + self.req_id_gen.fetch_add(2, std::sync::atomic::Ordering::Relaxed),
                )
            }
        }

        #[must_use]
        pub(super) fn register_req(&self, req_id_hash: NonZeroU64) -> RequestSlotId {
            let slot = RequestSlot { waker: AtomicWaker::new(), value: Mutex::new(None) };
            if self.waiters.insert(req_id_hash, slot).is_some() {
                panic!("Request ID collision")
            }
            RequestSlotId(req_id_hash)
        }

        /// Routes given response to appropriate handler. Returns `Err` if no handler is found.
        pub(super) fn route_response(
            &self,
            req_id_hash: NonZeroU64,
            response: msg::Response,
        ) -> Result<(), msg::Response> {
            let Some(slot) = self.waiters.get(&req_id_hash) else {
                return Err(response);
            };

            let mut value = slot.value.lock();
            value.replace(response);
            slot.waker.wake();

            Ok(())
        }

        /// Wake up all waiters. This is to abort all pending response futures that are waiting on
        /// closed connections. This doesn't do more than that, as the request context itself can
        /// be shared among multiple connections.
        pub(super) fn wake_up_all(&self) {
            for x in self.waiters.iter() {
                x.waker.wake();
            }
        }
    }

    /// When dropped, the response handler will be unregistered from the queue.
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    #[derive(Debug)]
    pub struct ResponseFuture<'a>(ResponseFutureInner<'a>);

    #[must_use = "futures do nothing unless you `.await` or poll them"]
    #[derive(Debug)]
    pub struct OwnedResponseFuture(ResponseFuture<'static>);

    #[derive(Debug)]
    enum ResponseFutureInner<'a> {
        Waiting(Cow<'a, Arc<dyn Connection>>, NonZeroU64),
        Finished,
    }

    impl<'a> ResponseFuture<'a> {
        pub(super) fn new(handle: &'a Arc<dyn Connection>, slot_id: RequestSlotId) -> Self {
            Self(ResponseFutureInner::Waiting(Cow::Borrowed(handle), slot_id.0))
        }

        pub fn to_owned(mut self) -> OwnedResponseFuture {
            let state = replace(&mut self.0, ResponseFutureInner::Finished);

            match state {
                ResponseFutureInner::Waiting(conn, id) => OwnedResponseFuture(ResponseFuture(
                    ResponseFutureInner::Waiting(Cow::Owned(conn.into_owned()), id),
                )),
                ResponseFutureInner::Finished => {
                    OwnedResponseFuture(ResponseFuture(ResponseFutureInner::Finished))
                }
            }
        }

        pub fn try_recv(&mut self) -> Result<Option<msg::Response>, RecvError> {
            use ResponseFutureInner::*;

            match &mut self.0 {
                Waiting(conn, hash) => {
                    if conn.__is_disconnected() {
                        // Let the 'drop' trait erase the request from the queue.
                        return Err(RecvError::Disconnected);
                    }

                    let mut value = None;
                    conn.reqs().unwrap().waiters.remove_if(hash, |_, elem| {
                        if let Some(v) = elem.value.lock().take() {
                            value = Some(v);
                            true
                        } else {
                            false
                        }
                    });

                    if value.is_some() {
                        self.0 = Finished;
                    }

                    Ok(value)
                }

                Finished => Ok(None),
            }
        }
    }

    impl<'a> std::future::Future for ResponseFuture<'a> {
        type Output = Result<msg::Response, RecvError>;

        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            use ResponseFutureInner::*;

            match &mut self.0 {
                Waiting(conn, hash) => {
                    if conn.__is_disconnected() {
                        // Let the 'drop' trait erase the request from the queue.
                        return Poll::Ready(Err(RecvError::Disconnected));
                    }

                    let mut value = None;
                    conn.reqs().unwrap().waiters.remove_if(hash, |_, elem| {
                        if let Some(v) = elem.value.lock().take() {
                            value = Some(v);
                            true
                        } else {
                            elem.waker.register(cx.waker());
                            false
                        }
                    });

                    if let Some(value) = value {
                        self.0 = Finished;
                        Poll::Ready(Ok(value))
                    } else {
                        Poll::Pending
                    }
                }

                Finished => panic!("ResponseFuture polled after completion"),
            }
        }
    }

    impl std::future::Future for OwnedResponseFuture {
        type Output = Result<msg::Response, RecvError>;

        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            self.0.poll_unpin(cx)
        }
    }

    impl futures_util::future::FusedFuture for ResponseFuture<'_> {
        fn is_terminated(&self) -> bool {
            matches!(self.0, ResponseFutureInner::Finished)
        }
    }

    impl futures_util::future::FusedFuture for OwnedResponseFuture {
        fn is_terminated(&self) -> bool {
            self.0.is_terminated()
        }
    }

    impl std::ops::Deref for OwnedResponseFuture {
        type Target = ResponseFuture<'static>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl std::ops::DerefMut for OwnedResponseFuture {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }

    impl Drop for ResponseFuture<'_> {
        fn drop(&mut self) {
            let state = std::mem::replace(&mut self.0, ResponseFutureInner::Finished);
            let ResponseFutureInner::Waiting(conn, hash) = state else { return };
            let reqs = conn.reqs().unwrap();
            assert!(
                reqs.waiters.remove(&hash).is_some(),
                "Request lifespan must be bound to this future."
            );
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

//  - Create async worker task to handle receive
pub struct Builder<Tw, Tr, C, E, R, U> {
    codec: C,
    write: Tw,
    read: Tr,
    ev: E,
    reqs: R,
    user_data: U,

    /// Required configurations
    cfg: BuilderOtherConfig,
}

#[derive(Default)]
struct BuilderOtherConfig {
    inbound_channel_cap: Option<NonZeroUsize>,
    feature_flag: Feature,
}

impl Default for Builder<(), (), (), EmptyEventListener, (), ()> {
    fn default() -> Self {
        Self {
            codec: (),
            write: (),
            read: (),
            cfg: Default::default(),
            ev: EmptyEventListener,
            reqs: (),
            user_data: (),
        }
    }
}

impl<Tw, Tr, C, E, R, U> Builder<Tw, Tr, C, E, R, U> {
    /// Specify codec to use.
    pub fn with_codec<C1: Codec>(
        self,
        codec: impl Into<Arc<C1>>,
    ) -> Builder<Tw, Tr, Arc<C1>, E, R, U> {
        Builder {
            codec: codec.into(),
            write: self.write,
            read: self.read,
            cfg: self.cfg,
            ev: self.ev,
            reqs: self.reqs,
            user_data: self.user_data,
        }
    }

    /// Specify write frame to use
    pub fn with_write<Tw2>(self, write: Tw2) -> Builder<Tw2, Tr, C, E, R, U>
    where
        Tw2: AsyncFrameWrite,
    {
        Builder {
            codec: self.codec,
            write,
            read: self.read,
            cfg: self.cfg,
            ev: self.ev,
            reqs: self.reqs,
            user_data: self.user_data,
        }
    }

    /// Specify [`AsyncFrameRead`] to use
    pub fn with_read<Tr2>(self, read: Tr2) -> Builder<Tw, Tr2, C, E, R, U>
    where
        Tr2: AsyncFrameRead,
    {
        Builder {
            codec: self.codec,
            write: self.write,
            read,
            cfg: self.cfg,
            ev: self.ev,
            reqs: self.reqs,
            user_data: self.user_data,
        }
    }

    /// Specify [`InboundEventSubscriber`] to use. This is used to handle errnous events from
    /// inbound driver
    pub fn with_event_listener<E2: InboundEventSubscriber>(
        self,
        ev: E2,
    ) -> Builder<Tw, Tr, C, E2, R, U> {
        Builder {
            codec: self.codec,
            write: self.write,
            read: self.read,
            cfg: self.cfg,
            ev,
            reqs: self.reqs,
            user_data: self.user_data,
        }
    }

    /// Specify addtional user data to store in the connection.
    pub fn with_user_data<U2: UserData>(self, user_data: U2) -> Builder<Tw, Tr, C, E, R, U2> {
        Builder {
            codec: self.codec,
            write: self.write,
            read: self.read,
            cfg: self.cfg,
            ev: self.ev,
            reqs: self.reqs,
            user_data,
        }
    }

    /// Set the read frame with stream reader and framing decoder.
    ///
    /// # Parameters
    ///
    /// - `default_readbuf_reserve`: When the framing decoder does not provide the next buffer size,
    ///   this value is used to pre-allocate the buffer for the next [`AsyncReadFrame::poll_read`]
    ///   call.
    pub fn with_read_stream<Tr2, F>(
        self,
        read: Tr2,
        framing: F,
        default_readbuf_reserve: usize,
    ) -> Builder<Tw, impl AsyncFrameRead, C, E, R, U>
    where
        Tr2: AsyncRead + Send + Sync + 'static,
        F: Framing,
    {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct FramingReader<T, F> {
            reader: T,
            framing: F,
            buf: bytes::BytesMut,
            nreserve: usize,
            state_skip_reading: bool,
        }

        impl<T, F> AsyncFrameRead for FramingReader<T, F>
        where
            T: AsyncRead + Sync + Send + 'static,
            F: Framing,
        {
            fn poll_next(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<Bytes>> {
                // SAFETY: We won't move this value, for sure, right?
                let FramingReader { reader, framing, buf, nreserve, state_skip_reading } =
                    unsafe { self.get_unchecked_mut() };

                let mut reader = unsafe { Pin::new_unchecked(reader) };

                loop {
                    // Read until the transport returns 'pending'
                    let size_hint = framing.next_buffer_size();
                    let size_required = size_hint.map(|x| x.get());

                    while !*state_skip_reading && size_required.is_some_and(|x| buf.len() < x) {
                        let n_req_size = size_required.unwrap_or(0).saturating_sub(buf.len());
                        let num_reserve = (*nreserve).max(n_req_size);

                        let old_cursor = buf.len();
                        buf.reserve(num_reserve);

                        unsafe {
                            buf.set_len(old_cursor + num_reserve);
                            match reader.as_mut().poll_read(cx, buf) {
                                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                                Poll::Ready(Ok(n)) => {
                                    buf.set_len(old_cursor + n);
                                }
                                Poll::Pending => {
                                    buf.set_len(old_cursor);

                                    // Skip reading until all prepared buffer is consumed.
                                    *state_skip_reading = true;
                                    break;
                                }
                            }
                        }
                    }

                    // Try decode the buffer.
                    match framing.try_framing(&buf[..]) {
                        Ok(Some(x)) => {
                            framing.advance();

                            debug_assert!(x.valid_data_end <= x.next_frame_start);
                            debug_assert!(x.next_frame_start <= buf.len());

                            let mut frame = buf.split_to(x.next_frame_start);
                            break Poll::Ready(Ok(frame.split_off(x.valid_data_end).into()));
                        }

                        // Just poll once more, until it retunrs 'Pending' ...
                        Ok(None) => {
                            *state_skip_reading = false;
                            continue;
                        }

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
            read: FramingReader {
                reader: read,
                framing,
                buf: BytesMut::default(),
                nreserve: default_readbuf_reserve,
                state_skip_reading: false,
            },
            cfg: self.cfg,
            ev: self.ev,
            reqs: self.reqs,
            user_data: self.user_data,
        }
    }

    /// Enable request features with default request context.
    pub fn with_request(self) -> Builder<Tw, Tr, C, E, RequestContext, U> {
        Builder {
            codec: self.codec,
            write: self.write,
            read: self.read,
            cfg: self.cfg,
            ev: self.ev,
            reqs: RequestContext::default(),
            user_data: self.user_data,
        }
    }

    /// Enable request features with custom request context. e.g. you can use
    /// [`Arc<RequestContext>`] to share the request context between multiple connections.
    pub fn with_request_context(
        self,
        reqs: impl GetRequestContext,
    ) -> Builder<Tw, Tr, C, E, impl GetRequestContext, U> {
        Builder {
            codec: self.codec,
            write: self.write,
            read: self.read,
            cfg: self.cfg,
            ev: self.ev,
            reqs,
            user_data: self.user_data,
        }
    }

    /// Setting zero will create unbounded channel. Default is unbounded.
    pub fn with_inbound_channel_capacity(mut self, capacity: usize) -> Self {
        self.cfg.inbound_channel_cap = capacity.try_into().ok();
        self
    }

    /// Enable specified feature flags. This applies bit-or-assign operation on feature flags.
    pub fn with_feature(mut self, feature: Feature) -> Self {
        self.cfg.feature_flag |= feature;
        self
    }

    /// Disable specified feature flags.
    pub fn without_feature(mut self, feature: Feature) -> Self {
        self.cfg.feature_flag &= !feature;
        self
    }
}

impl<Tw, Tr, C, E, R, U> Builder<Tw, Tr, Arc<C>, E, R, U>
where
    Tw: AsyncFrameWrite,
    Tr: AsyncFrameRead,
    C: Codec,
    E: InboundEventSubscriber,
    R: GetRequestContext,
    U: UserData,
{
    /// Build the connection from provided parameters.
    ///
    /// To start the connection, you need to spawn the returned future to the executor.
    #[must_use = "The connection will be closed immediately if you don't spawn the future!"]
    pub fn build(self) -> (Transceiver, impl std::future::Future<Output = ()> + Send) {
        let (tx_inb_drv, rx_inb_drv) = flume::unbounded();
        let (tx_in_msg, rx_in_msg) =
            if let Some(chan_cap) = self.cfg.inbound_channel_cap.map(|x| x.get()) {
                flume::bounded(chan_cap)
            } else {
                flume::unbounded()
            };

        let conn: Arc<dyn Connection>;
        let this = ConnectionImpl {
            codec: self.codec,
            write: AsyncMutex::new(self.write),
            reqs: self.reqs,
            tx_drive: tx_inb_drv,
            features: self.cfg.feature_flag,
            user_data: self.user_data,
            _unpin: std::marker::PhantomPinned,
        };

        let this = Arc::new(this);
        let fut_driver = ConnectionImpl::inbound_event_handler(DriverBody {
            w_this: Arc::downgrade(&this),
            read: self.read,
            ev_subs: self.ev,
            rx_drive: rx_inb_drv,
            tx_msg: tx_in_msg,
        });

        conn = this;
        (Transceiver(Sender(conn), rx_in_msg), fut_driver)
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

    #[error("Response hash was restored as 0, which is invalid.")]
    ResponseHashZero(msg::Response),

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
pub struct EmptyEventListener;

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
    fn req_id(&self) -> codec::ReqIdRef;
}

pub trait MessageMethodName: Message {
    fn method_raw(&self) -> &[u8];
    fn method(&self) -> Option<&str> {
        std::str::from_utf8(self.method_raw()).ok()
    }
}

pub trait MessageUserData {
    fn user_data_raw(&self) -> &dyn UserData;
    fn user_data_owned(&self) -> OwnedUserData;
    fn user_data<T: UserData>(&self) -> Option<&T> {
        self.user_data_raw().as_any().downcast_ref()
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
        codec::{DecodeError, EncodeError, PredefinedResponseError, ReqId, ReqIdRef},
        rpc::MessageReqId,
    };

    use super::{Connection, DeferredWrite, Message, MessageUserData, UserData, WriteBuffer};

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
                fn req_id(&self) -> ReqIdRef {
                    self.req_id.make_ref(&self.h.buffer)
                }
            }
        };
    }

    /* ------------------------------------- Request Logics ------------------------------------- */
    #[derive(Debug)]
    pub struct RequestInner {
        pub(super) h: super::InboundBody,
        pub(super) method: Range<usize>,
        pub(super) req_id: ReqId,
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

    impl MessageUserData for Request {
        fn user_data_raw(&self) -> &dyn UserData {
            self.body.as_ref().unwrap().1.user_data()
        }
        fn user_data_owned(&self) -> super::OwnedUserData {
            super::OwnedUserData(self.body.as_ref().unwrap().1.clone())
        }
    }

    impl Request {
        pub async fn response<T: serde::Serialize>(
            self,
            value: Result<&T, &T>,
        ) -> Result<(), super::SendError> {
            let mut buf = Default::default();
            self.response_with_reuse(&mut buf, value).await
        }

        /// Response with given value. If the value is `Err`, the response will be sent as error
        #[doc(hidden)]
        pub async fn response_with_reuse<T: serde::Serialize>(
            self,
            buf: &mut super::WriteBuffer,
            value: Result<&T, &T>,
        ) -> Result<(), super::SendError> {
            let conn = self.prepare_response(buf, value)?;
            conn.__write_raw(&mut buf.value).await?;
            Ok(())
        }

        /// Explicitly abort the request. This is useful when you want to cancel the request
        pub async fn abort(self) -> Result<(), super::SendError> {
            self.error_predefined(PredefinedResponseError::Aborted).await
        }

        /// Notify handler received notify handler ...
        pub async fn error_notify_handler(self) -> Result<(), super::SendError> {
            self.error_predefined(PredefinedResponseError::NotifyHandler).await
        }

        /// Response with 'parse failed' predefined error type.
        pub async fn error_parse_failed<T>(self) -> Result<(), super::SendError> {
            let err = PredefinedResponseError::ParseFailed(std::any::type_name::<T>().into());
            self.error_predefined(err).await
        }

        /// Response 'internal' error with given error code.
        pub async fn error_internal(
            self,
            errc: i32,
            detail: impl Into<Option<String>>,
        ) -> Result<(), super::SendError> {
            let err = if let Some(detail) = detail.into() {
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

        /* ---------------------------------- Deferred Version ---------------------------------- */
        #[doc(hidden)]
        pub fn response_deferred_with_reuse<T: serde::Serialize>(
            self,
            buffer: &mut WriteBuffer,
            value: Result<&T, &T>,
        ) -> Result<(), super::SendError> {
            buffer.prepare();
            let conn = self.prepare_response(buffer, value).unwrap();

            conn.tx_drive()
                .send(super::InboundDriverDirective::DeferredWrite(
                    DeferredWrite::Raw(buffer.value.split()).into(),
                ))
                .map_err(|_| super::SendError::Disconnected)
        }

        pub fn response_deferred<T: serde::Serialize>(
            self,
            value: Result<&T, &T>,
        ) -> Result<(), super::SendError> {
            self.response_deferred_with_reuse(&mut Default::default(), value)
        }

        pub fn abort_deferred(self) -> Result<(), super::SendError> {
            self.error_predefined_deferred(PredefinedResponseError::Aborted)
        }

        pub fn error_notify_handler_deferred(self) -> Result<(), super::SendError> {
            self.error_predefined_deferred(PredefinedResponseError::NotifyHandler)
        }

        pub fn error_parse_failed_deferred<T>(self) -> Result<(), super::SendError> {
            let err = PredefinedResponseError::ParseFailed(std::any::type_name::<T>().into());
            self.error_predefined_deferred(err)
        }

        pub fn error_internal_deferred(
            self,
            errc: i32,
            detail: impl Into<Option<String>>,
        ) -> Result<(), super::SendError> {
            let err = if let Some(detail) = detail.into() {
                PredefinedResponseError::InternalDetailed(errc, detail)
            } else {
                PredefinedResponseError::Internal(errc)
            };

            self.error_predefined_deferred(err)
        }

        fn error_predefined_deferred(
            mut self,
            err: super::codec::PredefinedResponseError,
        ) -> Result<(), super::SendError> {
            let (inner, conn) = self.body.take().unwrap();
            conn.tx_drive()
                .send(super::InboundDriverDirective::DeferredWrite(
                    DeferredWrite::ErrorResponse(inner, err).into(),
                ))
                .map_err(|_| super::SendError::Disconnected)
        }

        /* ------------------------------------ Inner Methods ----------------------------------- */
        fn prepare_response<T: serde::Serialize>(
            mut self,
            buf: &mut super::WriteBuffer,
            value: Result<&T, &T>,
        ) -> Result<Arc<dyn Connection>, EncodeError> {
            let (inner, conn) = self.body.take().unwrap();
            buf.prepare();

            let encode_as_error = value.is_err();
            let value = value.unwrap_or_else(|x| x);
            inner.codec().encode_response(
                inner.req_id(),
                encode_as_error,
                value,
                &mut buf.value,
            )?;

            Ok(conn.clone())
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
        pub(super) sender: Arc<dyn Connection>,
    }

    impl_message!(Notify);
    impl_method_name!(Notify);

    impl MessageUserData for Notify {
        fn user_data_raw(&self) -> &dyn UserData {
            self.sender.user_data()
        }

        fn user_data_owned(&self) -> super::OwnedUserData {
            super::OwnedUserData(self.sender.clone())
        }
    }

    /* ---------------------------------- Received Message Type --------------------------------- */

    #[derive(Debug, EnumAsInner)]
    pub enum RecvMsg {
        Request(Request),
        Notify(Notify),
    }

    impl MessageUserData for RecvMsg {
        fn user_data_raw(&self) -> &dyn UserData {
            match self {
                Self::Request(x) => x.user_data_raw(),
                Self::Notify(x) => x.user_data_raw(),
            }
        }

        fn user_data_owned(&self) -> super::OwnedUserData {
            match self {
                Self::Request(x) => x.user_data_owned(),
                Self::Notify(x) => x.user_data_owned(),
            }
        }
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
        pub(super) req_id: ReqId,

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
        ) -> Result<T, ResponseError<E>> {
            if !self.is_error {
                Ok(self.parse()?)
            } else {
                let codec = self.codec();
                if let Some(predef) = codec.try_decode_predef_error(self.payload()) {
                    Err(ResponseError::Predefined(predef))
                } else {
                    Err(ResponseError::Typed(self.parse()?))
                }
            }
        }
    }

    #[derive(EnumAsInner, Debug, thiserror::Error)]
    pub enum ResponseError<T> {
        #[error("Requested type returned")]
        Typed(T),

        #[error("Predefined error returned: {0}")]
        Predefined(PredefinedResponseError),

        #[error("Decode error: {0}")]
        DecodeError(#[from] DecodeError),
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                      DRIVER IMPLEMENTATION                                     */
/* ---------------------------------------------------------------------------------------------- */

struct DriverBody<C, T, E, R, U, Tr> {
    w_this: Weak<ConnectionImpl<C, T, R, U>>,
    read: Tr,
    ev_subs: E,
    rx_drive: flume::Receiver<InboundDriverDirective>,
    tx_msg: flume::Sender<msg::RecvMsg>,
}

mod inner {
    //! Internal driver context. It receives the request from the connection, and dispatches it to
    //! the handler.

    use bytes::BytesMut;
    use capture_it::capture;
    use futures_util::{future::FusedFuture, FutureExt};
    use std::{future::poll_fn, num::NonZeroU64, sync::Arc};

    use crate::{
        codec::{self, Codec, InboundFrameType},
        rpc::{DeferredWrite, SendError},
        transport::{AsyncFrameRead, AsyncFrameWrite, FrameReader},
    };

    use super::{
        msg, ConnectionImpl, DriverBody, Feature, GetRequestContext, InboundBody,
        InboundDriverDirective, InboundError, InboundEventSubscriber, MessageReqId, UserData,
    };

    /// Request-acceting version of connection driver
    impl<C, T, R, U> ConnectionImpl<C, T, R, U>
    where
        C: Codec,
        T: AsyncFrameWrite,
        R: GetRequestContext,
        U: UserData,
    {
        pub(crate) async fn inbound_event_handler<Tr, E>(body: DriverBody<C, T, E, R, U, Tr>)
        where
            Tr: AsyncFrameRead,
            E: InboundEventSubscriber,
        {
            body.execute().await;
        }
    }

    impl<C, T, E, R, U, Tr> DriverBody<C, T, E, R, U, Tr>
    where
        C: Codec,
        T: AsyncFrameWrite,
        E: InboundEventSubscriber,
        R: GetRequestContext,
        U: UserData,
        Tr: AsyncFrameRead,
    {
        async fn execute(self) {
            let DriverBody { w_this, mut read, mut ev_subs, rx_drive: rx_msg, tx_msg } = self;

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
                    let Some(this) = w_this.upgrade() else { break };
                    match msg {
                        DeferredWrite::ErrorResponse(req, err) => {
                            let err = this.dyn_ref().__send_err_predef(&mut pool, &req, &err).await;

                            // Ignore all other error types, as this message is triggered
                            // crate-internally on very limited situations
                            if let Err(SendError::IoError(e)) = err {
                                return Err(e);
                            }
                        }
                        DeferredWrite::Raw(mut msg) => {
                            this.dyn_ref().__write_raw(&mut msg).await?;
                        }
                        DeferredWrite::Flush => {
                            this.dyn_ref().__flush().await?;
                        }
                    }
                }

                Ok::<(), std::io::Error>(())
            });
            let fut_bg_sender = fut_bg_sender.fuse();
            let mut fut_bg_sender = std::pin::pin!(fut_bg_sender);
            let mut read = std::pin::pin!(read);

            /* ------------------------------------ App Loop ------------------------------------ */
            loop {
                if fut_drive_msg.is_terminated() {
                    fut_drive_msg = rx_msg.recv_async().fuse();
                }

                if fut_read.is_terminated() {
                    fut_read = poll_fn(|cx| read.as_mut().poll_next(cx)).fuse();
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
                                    bytes,
                                    &tx_msg
                                )
                                .await;
                            }
                            Err(e) => {
                                close_from_remote = true;
                                ev_subs.on_close(false, Err(e));

                                break;
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
                ev_subs.on_close(true, Ok(())); // We're closing this
            }

            // Let all pending requests to be cancelled.
            'cancel: {
                let Some(this) = w_this.upgrade() else { break 'cancel };
                let Some(reqs) = this.reqs.get_req_con() else { break 'cancel };

                // Let the handle recognized as 'disconnected'
                drop(fut_drive_msg);
                drop(rx_msg);

                // Wake up all pending responses
                reqs.wake_up_all();
            }
        }

        async fn on_read(
            this: &Arc<ConnectionImpl<C, T, R, U>>,
            ev_subs: &mut E,
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
                            msg::RecvMsg::Notify(msg::Notify { h, method, sender: this.clone() }),
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
                    let response = msg::Response { h, is_error, req_id };

                    let Some(reqs) = this.reqs.get_req_con() else {
                        ev_subs.on_inbound_error(InboundError::RedundantResponse(response));
                        return;
                    };

                    let Some(req_id_hash) = NonZeroU64::new(req_id_hash) else {
                        ev_subs.on_inbound_error(InboundError::ResponseHashZero(response));
                        return;
                    };

                    if let Err(msg) = reqs.route_response(req_id_hash, response) {
                        ev_subs.on_inbound_error(InboundError::ExpiredResponse(msg));
                    }
                }
            }
        }
    }

    /// SAFETY: `ConnectionImpl` is `!Unpin`, thus it is safe to pin the reference.
    macro_rules! pin {
        ($this:ident, $ident:ident) => {
            let mut $ident = $this.write().lock().await;
            let mut $ident = unsafe { std::pin::Pin::new_unchecked(&mut *$ident) };
        };
    }

    impl dyn super::Connection {
        pub(crate) async fn __send_err_predef(
            &self,
            buf: &mut super::WriteBuffer,
            recv: &msg::RequestInner,
            error: &codec::PredefinedResponseError,
        ) -> Result<(), super::SendError> {
            buf.prepare();

            self.codec().encode_response_predefined(recv.req_id(), error, &mut buf.value)?;
            self.__write_raw(&mut buf.value).await?;
            Ok(())
        }

        pub(crate) async fn __write_raw(&self, buf: &mut BytesMut) -> std::io::Result<()> {
            pin!(self, write);

            write.as_mut().begin_write_frame(buf.len())?;
            let mut reader = FrameReader::new(buf);

            while !reader.is_empty() {
                poll_fn(|cx| write.as_mut().poll_write(cx, &mut reader)).await?;
            }

            Ok(())
        }

        pub(crate) async fn __flush(&self) -> std::io::Result<()> {
            pin!(self, write);
            poll_fn(|cx| write.as_mut().poll_flush(cx)).await
        }

        pub(crate) async fn __close(&self) -> std::io::Result<()> {
            pin!(self, write);
            poll_fn(|cx| write.as_mut().poll_close(cx)).await
        }

        pub(crate) fn __is_disconnected(&self) -> bool {
            self.tx_drive().is_disconnected()
        }
    }
}
