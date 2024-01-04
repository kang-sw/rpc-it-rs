use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Buf, Bytes};
use futures_core::Stream;
use tokio::io::AsyncWrite;

/// [`AsyncFrameWrite`] is a trait defining the interface for writing data frames
/// to the underlying transport. This trait can either be a straightforward wrapper
/// around the `AsyncWrite` interface or an optimized custom implementation.
/// It may collect [`Bytes`] and flush them in batches to minimize buffer copies.
pub trait AsyncFrameWrite: 'static + Send {
    /// Notifies the underlying transport about a new frame.
    fn start_frame(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    /// Writes a frame to the underlying transport.
    ///
    /// Type of the parameter `buffer` is reference to help this method to easily deal with
    /// remaining byte count in the buffer.
    fn poll_write_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut Bytes,
    ) -> Poll<std::io::Result<()>>;

    /// Flushes the underlying transport, writing any pending data.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>>;

    /// Closes the underlying transport.
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>>;
}

/// [`AsyncFrameRead`] is a trait defining the interface for reading data frames from the
/// underlying transport.
pub trait AsyncFrameRead {
    /// Reads a single frame. The returned frame must be a *complete* message that can be
    /// decoded by a single action. Therefore, the responsibility of framing inbound stream data
    /// lies with the implementation of [`AsyncFrameRead`].
    fn poll_read_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<Bytes>>;
}

// ==== AsyncFrameWrite ====

/// Implements [`AsyncFrameWrite`] for any type that implements [`tokio::io::AsyncWrite`].
impl<T> AsyncFrameWrite for T
where
    T: AsyncWrite + Send + 'static,
{
    fn poll_write_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut Bytes,
    ) -> Poll<std::io::Result<()>> {
        match AsyncWrite::poll_write(self, cx, buf.as_ref())? {
            Poll::Ready(x) => {
                buf.advance(x);
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_flush(self, cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_shutdown(self, cx)
    }
}

// ==== AsyncFrameRead ====

impl<T> AsyncFrameRead for T
where
    T: Stream<Item = std::io::Result<Bytes>> + Unpin + Send + 'static,
{
    fn poll_read_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<Bytes>> {
        match Stream::poll_next(self, cx) {
            Poll::Ready(Some(x)) => Poll::Ready(x),
            Poll::Ready(None) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}
