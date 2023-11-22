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
    num::{NonZeroU64, NonZeroUsize},
    ops::Range,
    sync::{Arc, Weak},
};

use crate::{
    codec::{self, Codec, DecodeError, EncodeError, Framing, FramingError},
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

impl From<DeferredWrite> for InboundDriverDirective {
    fn from(value: DeferredWrite) -> Self {
        Self::DeferredWrite(value)
    }
}

#[derive(Debug)]
enum DeferredWrite {
    /// Send error response to the request
    ErrorResponse(msg::RequestInner, codec::PredefinedResponseError),

    /// Send request was deferred. This is used for non-blocking response, etc.
    Raw(bytes::Bytes),

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
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        let fut = self.prepare_request(buf, method, params)?;
        self.0.__write_buffer(buf).await?;

        Ok(fut)
    }

    #[doc(hidden)]
    pub async fn notify_with_reuse<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        buf.clear();

        self.0.codec().encode_notify(method, params, buf)?;
        self.0.__write_buffer(buf).await?;

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
        buffer: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        buffer.clear();
        let fut = self.prepare_request(buffer, method, params);

        self.0
            .tx_drive()
            .send(DeferredWrite::Raw(buffer.split().freeze()).into())
            .map_err(|_| SendError::Disconnected)?;

        fut
    }

    /// Send deferred notification. This method is non-blocking, as the message writing will be
    /// deferred to the background driver worker.
    #[doc(hidden)]
    pub fn notify_deferred_with_reuse<T: serde::Serialize>(
        &self,
        buffer: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        buffer.clear();
        self.0.codec().encode_notify(method, params, buffer)?;

        self.0
            .tx_drive()
            .send(DeferredWrite::Raw(buffer.split().freeze()).into())
            .map_err(|_| SendError::Disconnected)
    }

    fn prepare_request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture, SendError> {
        let Some(req) = self.0.reqs() else { return Err(SendError::RequestDisabled) };

        buf.clear();
        let req_id_hint = req.next_req_id_base();
        let req_id_hash = self.0.codec().encode_request(method, req_id_hint, params, buf)?;

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
        self.0.tx_drive().send(DeferredWrite::Flush.into()).is_ok()
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

mod req;

/* ---------------------------------------------------------------------------------------------- */
/*                                        ERROR DEFINIITONS                                       */
/* ---------------------------------------------------------------------------------------------- */
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
/*                                      ENCODING NOTIFICATION                                     */
/* ---------------------------------------------------------------------------------------------- */

pub struct EncodedNotify {
    encode_hash: NonZeroU64,
    payload: bytes::Bytes,
}

impl dyn Codec {
    /// Prepare notification that is reusable across codecs which has same notification hash.
    pub fn prepare_notification<T: serde::Serialize>(
        &self,
        buffer: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<EncodedNotify, EncodeError> {
        let Some(hash) = self.notification_encoder_hash() else {
            return Err(EncodeError::PreparedNotificationNotSupported);
        };

        Ok(EncodedNotify {
            encode_hash: hash,
            payload: {
                buffer.clear();
                self.encode_notify(method, params, buffer)?;
                buffer.split().freeze()
            },
        })
    }
}

impl Sender {
    pub async fn send_prepared_notification(&self, msg: &EncodedNotify) -> Result<(), SendError> {
        self.verify_prepared_notification(msg.encode_hash)?;
        self.0.__write_bytes(&mut msg.payload.clone()).await?;
        Ok(())
    }

    pub fn send_prepared_notification_defferred(
        &self,
        msg: &EncodedNotify,
    ) -> Result<(), SendError> {
        self.verify_prepared_notification(msg.encode_hash)?;
        self.0
            .tx_drive()
            .send(DeferredWrite::Raw(msg.payload.clone()).into())
            .map_err(|_| SendError::Disconnected)
    }

    pub(crate) async fn write_bytes(&self, bytes: &mut Bytes) -> Result<(), SendError> {
        self.0.__write_bytes(bytes).await.map_err(Into::into)
    }

    pub(crate) fn write_bytes_deferred(&self, bytes: Bytes) -> Result<(), SendError> {
        self.0
            .tx_drive()
            .send(DeferredWrite::Raw(bytes).into())
            .map_err(|_| SendError::Disconnected)
    }

    fn verify_prepared_notification(&self, msg_hash: NonZeroU64) -> Result<(), EncodeError> {
        let Some(hash) = self.0.codec().notification_encoder_hash() else {
            return Err(EncodeError::PreparedNotificationNotSupported);
        };

        if hash != msg_hash {
            return Err(EncodeError::PreparedNotificationMismatch);
        }

        Ok(())
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

pub trait ExtractUserData {
    fn extract_sender(&self) -> Sender;
    fn user_data_raw(&self) -> &dyn UserData;
    fn user_data_owned(&self) -> OwnedUserData;
    fn user_data<T: UserData>(&self) -> Option<&T> {
        self.user_data_raw().as_any().downcast_ref()
    }
}

impl ExtractUserData for Sender {
    fn extract_sender(&self) -> Sender {
        self.clone()
    }

    fn user_data_raw(&self) -> &dyn UserData {
        self.0.user_data()
    }

    fn user_data_owned(&self) -> OwnedUserData {
        OwnedUserData(self.0.clone())
    }
}

/// Common content of inbound message
#[derive(Debug)]
struct InboundBody {
    buffer: Bytes,
    payload: Range<usize>,
    codec: Arc<dyn Codec>,
}

pub mod msg;

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

mod inner;
