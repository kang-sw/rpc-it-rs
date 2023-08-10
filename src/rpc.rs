use std::{fmt::Debug, num::NonZeroUsize, ops::Range, sync::Arc};

use crate::{
    codec::{DecodeError, FramingError},
    prelude::{Codec, Framing},
    transport::{AsyncRead, AsyncReadFrame, AsyncWriteFrame},
};

use async_lock::Mutex as AsyncMutex;
use bytes::{Bytes, BytesMut};
use smallvec::SmallVec;

/* ---------------------------------------------------------------------------------------------- */
/*                                            CONSTANTS                                           */
/* ---------------------------------------------------------------------------------------------- */
const FRAMING_READ_STREAM_DEFAULT_BUFFER_LENGTH: usize = 1024;

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
#[derive(Debug)]
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
#[derive(derive_setters::Setters)]
pub struct Builder<T0, T1, C> {
    #[setters(skip)]
    codec: C,
    #[setters(skip)]
    write: T0,
    #[setters(skip)]
    read: T1,
}

impl Default for Builder<(), (), ()> {
    fn default() -> Self {
        Self { codec: (), write: (), read: () }
    }
}

impl<T0, T1, C> Builder<T0, T1, C> {
    /// Specify codec to use
    pub fn with_codec<C1: Codec>(self, codec: C1) -> Builder<T0, T1, C1> {
        Builder { codec, write: self.write, read: self.read }
    }

    /// Specify write frame to use
    pub fn with_write<T2: AsyncWriteFrame>(self, write: T2) -> Builder<T2, T1, C> {
        Builder { codec: self.codec, write, read: self.read }
    }

    pub fn with_read<T2: AsyncReadFrame>(self, read: T2) -> Builder<T0, T2, C> {
        Builder { codec: self.codec, write: self.write, read }
    }

    /// Set the read frame with stream reader and framing decoder.
    ///
    /// # Parameters
    ///
    /// - `fallback_buffer_size`: When the framing decoder does not provide the next buffer size,
    ///   this value is used to pre-allocate the buffer for the next [`AsyncReadFrame::poll_read`]
    ///   call.
    pub fn with_read_stream<T2: AsyncRead, F: Framing>(
        self,
        read: T2,
        framing: F,
        fallback_buffer_size: Option<NonZeroUsize>,
    ) -> Builder<T0, impl AsyncReadFrame, C> {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct FramingReader<T, F> {
            reader: T,
            framing: F,
            cursor: usize,
            fb_n_buf: usize,
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
                    let buflen = this
                        .framing
                        .next_buffer_size()
                        .map(|x| x.get())
                        .unwrap_or_default()
                        .max(this.fb_n_buf);
                    let margin_now = buf.len() - this.cursor;
                    let n_alloc_more = buflen.saturating_sub(margin_now);
                    buf.reserve(n_alloc_more);

                    // SAFETY: We don't use the `unread` bytes
                    unsafe { buf.set_len(buf.capacity()) };
                    let buffer = &mut buf.as_mut()[this.cursor..];

                    let Poll::Ready(read_len) = Pin::new(&mut this.reader).poll_read(cx, buffer)? else {
                        break Poll::Pending;
                    };

                    debug_assert!(
                        read_len <= buflen,
                        "It returned larger length than requested buffer size"
                    );

                    this.cursor += read_len;
                    match this.framing.advance(&buf[..this.cursor]) {
                        Ok(Some(x)) => {
                            debug_assert!(x.valid_data_end <= x.next_frame_start);
                            debug_assert!(x.next_frame_start <= this.cursor);

                            this.cursor -= x.next_frame_start;

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
                            debug_assert!(num_discard_bytes <= this.cursor);

                            let _ = buf.split_to(num_discard_bytes);
                            this.cursor -= num_discard_bytes;
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
                cursor: 0,
                fb_n_buf: fallback_buffer_size
                    .map(|x| x.get())
                    .unwrap_or(FRAMING_READ_STREAM_DEFAULT_BUFFER_LENGTH),
            },
        }
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
}
