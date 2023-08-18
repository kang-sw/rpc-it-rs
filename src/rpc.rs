use std::{
    fmt::Debug,
    future::Future,
    num::NonZeroUsize,
    ops::Range,
    sync::{atomic::AtomicBool, Arc, Weak},
};

use crate::{
    codec::{self, Codec, DecodeError, Framing, FramingError},
    transport::{AsyncRead, AsyncReadFrame, AsyncWriteFrame},
};

use async_lock::Mutex as AsyncMutex;
use bytes::{Bytes, BytesMut};

/* ---------------------------------------------------------------------------------------------- */
/*                                            CONSTANTS                                           */
/* ---------------------------------------------------------------------------------------------- */

/* ---------------------------------------------------------------------------------------------- */
/*                                          BACKED TRAIT                                          */
/* ---------------------------------------------------------------------------------------------- */
/// Creates RPC connection from [`crate::transport::AsyncReadFrame`] and
/// [`crate::transport::AsyncWriteFrame`], and [`crate::codec::Codec`].
///
/// For unsupported features(e.g. notify from client), the codec should return
/// [`crate::codec::EncodeError::UnsupportedFeature`] error.
struct ConnectionImpl<C, T, R> {
    codec: C,
    write: AsyncMutex<T>,
    reqs: R,
    drive: flume::Sender<InboundDriverDirective>,
    is_alive: AtomicBool,
}

/// Wraps connection implementation with virtual dispatch.
trait Connection: Send + Sync + 'static + Debug {
    fn codec(&self) -> &dyn Codec;
    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame>;
    fn reqs(&self) -> Option<&RequestContext>;
    fn drive(&self) -> &flume::Sender<InboundDriverDirective>;
    fn is_alive(&self) -> bool;
}

impl<C, T, R> Connection for ConnectionImpl<C, T, R>
where
    C: Codec,
    T: AsyncWriteFrame,
    R: GetRequestContext,
{
    fn codec(&self) -> &dyn Codec {
        &self.codec
    }

    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame> {
        &self.write
    }

    fn reqs(&self) -> Option<&RequestContext> {
        self.reqs.get_req_con()
    }

    fn drive(&self) -> &flume::Sender<InboundDriverDirective> {
        &self.drive
    }

    fn is_alive(&self) -> bool {
        self.is_alive.load(std::sync::atomic::Ordering::Acquire)
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
            drive: self.drive,
            is_alive: self.is_alive,
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

/// RPC request context. Stores request ID and response receiver context.
#[derive(Debug, Default)]
struct RequestContext {
    // TODO: Registered request futures ...
    // TODO: Handle cancellations ...
}

/// Which couldn't be handled within the non-async drop handlers ...
///
/// - A received request object is dropped before response is sent, therefore an 'aborted' message
///   should be sent to the receiver
#[derive(Debug)]
enum InboundDriverDirective {
    DeferredWrite(DeferredWrite),
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
pub struct BufferPool {
    tmpbuf: Vec<u8>,
}

impl SendHandle {
    pub async fn send_request<'a, T: serde::Serialize, R>(
        &'a self,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture<R>, SendError> {
        self.send_request_ex(&mut Default::default(), method, params).await
    }

    pub async fn send_notify<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        self.send_notify_ex(&mut Default::default(), method, params).await
    }

    #[doc(hidden)]
    pub async fn send_request_ex<'a, T: serde::Serialize, R>(
        &'a self,
        buf_reuse: &mut BufferPool,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture<R>, SendError> {
        todo!()
    }

    #[doc(hidden)]
    pub async fn send_notify_ex<T: serde::Serialize>(
        &self,
        buf_reuse: &mut BufferPool,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        todo!()
    }

    pub fn is_closed(&self) -> bool {
        // TODO:
        false
    }

    pub async fn close(self) -> std::io::Result<()> {
        todo!()
    }
}

/// For response, there is no specific 'timeout' on waiting. As soon as we drop this future, the
/// response registration also be unregistered from queue.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ResponseFuture<T> {
    _type: std::marker::PhantomData<T>,
    handle: SendHandle,
    req_id: u64,
}

/// Common content of inbound message
#[derive(Debug)]
struct InboundBody {
    buffer: Bytes,
    payload: Range<usize>,
}

pub trait Inbound: Sized {
    #[doc(hidden)]
    fn inbound_get_handle(&self) -> &SendHandle;

    fn method(&self) -> Option<&str>;
    fn payload(&self) -> &[u8];

    fn decode_in_place<'a, T: serde::Deserialize<'a>>(
        &'a self,
        dst: &mut Option<T>,
    ) -> Result<(), DecodeError> {
        if let Some(dst) = dst.as_mut() {
        } else {
            self.inbound_get_handle().0.codec().decode_payload(self.payload(), &mut |de| {
                *dst = Some(erased_serde::deserialize(de)?);
                Ok(())
            })?;
        }

        Ok(())
    }

    fn decode<'a, T: serde::Deserialize<'a>>(&'a self) -> Result<T, DecodeError> {
        let mut dst = None;
        self.decode_in_place(&mut dst)?;
        Ok(dst.unwrap())
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
            codec: self.codec,
            write: AsyncMutex::new(self.write),
            reqs: (),
            drive: tx_inb_drv,
            is_alive: AtomicBool::new(true),
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
    ResponseWithoutRequest,

    #[error("Response packet is not routed.")]
    ResponseNotFound(msg::Response),
    // TODO: ParseFailed
    // TODO: BrokenStream
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
        let _ = error;
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
    fn into_raw(self) -> Bytes;
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

pub mod msg {
    use std::ops::Range;

    // TODO: Message Trait Implementations

    macro_rules! impl_message {
        ($t:ty) => {
            impl super::Message for $t {
                fn payload(&self) -> &[u8] {
                    &self.h.buffer[self.h.payload.clone()]
                }

                fn raw(&self) -> &[u8] {
                    &self.h.buffer
                }

                fn into_raw(self) -> bytes::Bytes {
                    self.h.buffer
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

    impl_message!(RequestInner);
    impl_method_name!(RequestInner);
    impl_req_id!(RequestInner);

    impl_message!(Notify);
    impl_method_name!(Notify);

    impl_message!(Response);
    impl_req_id!(Response);

    #[derive(Debug)]
    pub struct RequestInner {
        h: super::InboundBody,
        method: Range<usize>,
        req_id: Range<usize>,
    }

    #[derive(Debug)]
    pub struct Request {
        inner: RequestInner,
        w: super::SendHandle,
    }

    impl std::ops::Deref for Request {
        type Target = RequestInner;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    #[derive(Debug)]
    pub struct Notify {
        h: super::InboundBody,
        method: Range<usize>,
    }

    #[derive(Debug)]
    pub struct Response {
        h: super::InboundBody,
        req_id: Range<usize>,

        /// Should we interpret the payload as error object?
        is_error: bool,
    }

    #[derive(Debug)]
    pub enum RecvMsg {
        Request(Request),
        Notify(Notify),
    }

    // TODO: Inbound impls
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
    //!
    // TODO: Driver implementation

    use bytes::BytesMut;
    use capture_it::capture;
    use futures_util::{future::FusedFuture, FutureExt};
    use std::{future::poll_fn, pin::Pin, sync::Weak};
    use with_drop::with_drop;

    use crate::{
        codec::{self, Codec},
        rpc::{Connection, DeferredWrite, SendError},
        transport::{AsyncReadFrame, AsyncWriteFrame},
    };

    use super::{
        msg, ConnectionImpl, DriverBody, GetRequestContext, InboundDriverDirective,
        InboundEventSubscriber, RequestContext,
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
                todo!("Route response to awaiting request handlers");
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
            body.execute(|_, _, _| ()).await
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
            fn_handle_response: impl Fn(&ConnectionImpl<C, T, R>, &mut E, HandleResponseParam<'_>),
        ) {
            let DriverBody { w_this, mut read, mut ev_subs, rx_drive: rx_msg, tx_msg } = self;
            let mut rx_buf = bytes::BytesMut::new();

            use futures_util::future::Fuse;
            let mut fut_drive_msg = Fuse::terminated();
            let mut fut_read = Fuse::terminated();
            let mut close_from_remote = false;

            /* ----------------------------- Background Sender Task ----------------------------- */
            // Tasks that perform write operations in the background. For example, within the `Drop`
            // trait, which does not allow asynchronous operations, it can be used to send a cancel
            // handling of an `unhandled request`.
            let (tx_bg_sender, rx_bg_sender) = flume::unbounded();
            let fut_bg_sender = capture!([w_this], async move {
                let mut pool = super::BufferPool::default();
                while let Ok(msg) = rx_bg_sender.recv_async().await {
                    match msg {
                        DeferredWrite::ErrorResponse(req, err) => {
                            let Some(this) = w_this.upgrade() else { break };
                            let err = this.dyn_ref().__send_error_rep(&mut pool, &req, &err).await;

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

            // Mark the connection as 'dead' when the driver is terminated.
            let _mark_dead_on_exit = with_drop(w_this.clone(), |x| {
                let Some(x) = x.upgrade() else { return };
                x.is_alive.store(false, std::sync::atomic::Ordering::Release);
            });

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

                            Err(_) => {
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
                                Self::on_read(&this, &mut ev_subs, &fn_handle_response, bytes, &tx_msg).await;
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
            this: &ConnectionImpl<C, T, R>,
            ev_subs: &mut E,
            fn_handle_response: &impl Fn(&ConnectionImpl<C, T, R>, &mut E, HandleResponseParam<'_>),
            inbound: bytes::Bytes,
            tx_msg: &flume::Sender<msg::RecvMsg>,
        ) {
            // TODO: Handle inbound frame
        }
    }

    impl dyn super::Connection {
        pub(crate) async fn __send_error_rep(
            &self,
            buf: &mut super::BufferPool,
            recv: &msg::RequestInner,
            error: &codec::PredefinedResponseError,
        ) -> Result<(), super::SendError> {
            todo!();
        }

        pub(crate) async fn __write_raw(&self, buf: &[u8]) -> std::io::Result<()> {
            let mut write = self.write().lock().await;
            poll_fn(|cx| Pin::new(&mut *write).poll_pre_write(cx)).await?;
            poll_fn(|cx| Pin::new(&mut *write).poll_write(cx, buf)).await?;
            Ok(())
        }

        pub(crate) async fn __close(&self) -> std::io::Result<()> {
            todo!()
        }
    }

    struct HandleResponseParam<'a> {
        payload: bytes::Bytes,
        req_id: &'a [u8],
        is_error: bool,
    }
}
