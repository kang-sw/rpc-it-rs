use std::{fmt::Debug, future::Future, num::NonZeroUsize, ops::Range, sync::Arc};

use crate::{
    codec::{Codec, DecodeError, Framing, FramingError},
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
struct ConnectionBody<C, T, R> {
    codec: C,
    write: AsyncMutex<T>,
    reqs: R,
}

trait Connection: Send + Sync + 'static + Debug {
    fn codec(&self) -> &dyn Codec;
    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame>;
    fn reqs(&self) -> Option<&RequestContext>;
}

impl<C, T> Connection for ConnectionBody<C, T, RequestContext>
where
    C: Codec,
    T: AsyncWriteFrame,
{
    fn codec(&self) -> &dyn Codec {
        &self.codec
    }

    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame> {
        &self.write
    }

    fn reqs(&self) -> Option<&RequestContext> {
        Some(&self.reqs)
    }
}

impl<C, T> Connection for ConnectionBody<C, T, ()>
where
    C: Codec,
    T: AsyncWriteFrame,
{
    fn codec(&self) -> &dyn Codec {
        &self.codec
    }

    fn write(&self) -> &AsyncMutex<dyn AsyncWriteFrame> {
        &self.write
    }

    fn reqs(&self) -> Option<&RequestContext> {
        None
    }
}

impl<C, T, R: Debug> Debug for ConnectionBody<C, T, R> {
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
pub struct ReusableSendBuffer {
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
        buf_reuse: &mut ReusableSendBuffer,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture<R>, SendError> {
        todo!()
    }

    #[doc(hidden)]
    pub async fn send_notify_ex<T: serde::Serialize>(
        &self,
        buf_reuse: &mut ReusableSendBuffer,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
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

#[derive(Debug)]
struct InboundBody {
    buffer: Bytes,
    handle: SendHandle,
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

    #[error("Error during sending encoded message: {0}")]
    SendFailed(std::io::Error),

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
pub struct Builder<Tw, Tr, C> {
    codec: C,
    write: Tw,
    read: Tr,

    /// Required configurations
    cfg: BuilderOtherConfig,
}

#[derive(Default)]
struct BuilderOtherConfig {
    enable_send_request: bool,
    inbound_channel_cap: Option<NonZeroUsize>,
}

impl Default for Builder<(), (), ()> {
    fn default() -> Self {
        Self { codec: (), write: (), read: (), cfg: Default::default() }
    }
}

impl<Tw, Tr, C> Builder<Tw, Tr, C> {
    /// Specify codec to use
    pub fn with_codec<C1: Codec>(self, codec: C1) -> Builder<Tw, Tr, C1> {
        Builder { codec, write: self.write, read: self.read, cfg: self.cfg }
    }

    /// Specify write frame to use
    pub fn with_write<Tw2: AsyncWriteFrame>(self, write: Tw2) -> Builder<Tw2, Tr, C> {
        Builder { codec: self.codec, write, read: self.read, cfg: self.cfg }
    }

    pub fn with_read<Tr2: AsyncReadFrame>(self, read: Tr2) -> Builder<Tw, Tr2, C> {
        Builder { codec: self.codec, write: self.write, read, cfg: self.cfg }
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
    ) -> Builder<Tw, impl AsyncReadFrame, C> {
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

impl<Tw, Tr, C> Builder<Tw, Tr, C>
where
    Tw: AsyncWriteFrame,
    Tr: AsyncReadFrame,
    C: Codec,
{
    pub fn build(self) -> Result<(Handle, impl Future), std::io::Error> {
        let mut fut_with_request = None;
        let mut fut_without_request = None;

        let (tx_in_msg, rx_in_msg) =
            if let Some(chan_cap) = self.cfg.inbound_channel_cap.map(|x| x.get()) {
                flume::bounded(chan_cap)
            } else {
                flume::unbounded()
            };

        let conn: Arc<dyn Connection>;

        if self.cfg.enable_send_request {
            let arc = Arc::new(ConnectionBody {
                codec: self.codec,
                write: AsyncMutex::new(self.write),
                reqs: RequestContext::default(),
            });

            fut_with_request = Some(<ConnectionBody<_, _, RequestContext>>::inbound_event_handler(
                Arc::downgrade(&arc),
                self.read,
                tx_in_msg,
            ));

            conn = arc;
        } else {
            let arc = Arc::new(ConnectionBody {
                codec: self.codec,
                write: AsyncMutex::new(self.write),
                reqs: (),
            });

            fut_without_request = Some(<ConnectionBody<_, _, ()>>::inbound_event_handler(
                Arc::downgrade(&arc),
                self.read,
                tx_in_msg,
            ));

            conn = arc;
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
/*                                            MESSAGES                                            */
/* ---------------------------------------------------------------------------------------------- */
pub mod msg {
    use std::ops::Range;

    #[derive(Debug)]
    pub struct Request {
        h: super::InboundBody,
        method: Range<usize>,
        req_id: Range<usize>,
    }

    #[derive(Debug)]
    pub struct Notify {
        h: super::InboundBody,
        method: Range<usize>,
    }

    #[derive(Debug)]
    pub struct Response {
        h: super::InboundBody,
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
mod driver {
    //! Internal driver context. It receives the request from the connection, and dispatches it to
    //! the handler.
    //!
    // TODO: Driver implementation

    use std::sync::Weak;

    use bytes::BytesMut;

    use crate::{
        codec::Codec,
        transport::{AsyncReadFrame, AsyncWriteFrame},
    };

    use super::{msg, ConnectionBody, RequestContext};

    struct InboundDriverCtx<C, T, R> {
        conn: Weak<ConnectionBody<C, T, R>>,
        tx_msg: flume::Sender<msg::RecvMsg>,

        recv_buf: BytesMut,
    }

    /// Request-acceting version of connection driver
    impl<C, T> ConnectionBody<C, T, RequestContext>
    where
        C: Codec,
        T: AsyncWriteFrame,
    {
        pub(crate) async fn inbound_event_handler<Tr: AsyncReadFrame>(
            w_this: Weak<Self>,
            read: Tr,
            tx_recv_msg: flume::Sender<msg::RecvMsg>,
        ) {
        }
    }

    /// Non-request-accepting version of connection driver
    impl<C, T> ConnectionBody<C, T, ()>
    where
        C: Codec,
        T: AsyncWriteFrame,
    {
        pub(crate) async fn inbound_event_handler<Tr: AsyncReadFrame>(
            w_this: Weak<Self>,
            read: Tr,
            tx_recv_msg: flume::Sender<msg::RecvMsg>,
        ) {
        }
    }

    /// Puts shared logic between both version of connection drivers
    impl<C, T, R> ConnectionBody<C, T, R>
    where
        C: Codec,
        T: AsyncWriteFrame,
    {
        async fn recv_inbound_once<Tr: AsyncReadFrame>(w_this: &Weak<Self>, read: &mut Tr) {}
    }
}
