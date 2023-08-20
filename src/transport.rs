use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub use bytes::Bytes;
use futures_util::{AsyncWrite, Stream};

pub trait AsyncFrameWrite: Send + 'static {
    /// Called before writing a frame. This can be used to deal with writing cancellation.
    fn begin_write_frame(self: Pin<&mut Self>, len: usize) -> std::io::Result<()> {
        let _ = (len,);
        Ok(())
    }

    /// Write a frame to the underlying transport. It can be called multiple times to write a
    /// single frame. In this case, the input buffer is subspan of original frame, which was
    /// advanced by the return byte count of the previous call.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>>;

    /// Flush the underlying transport.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>>;

    /// Close the underlying transport.
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>>;
}

/// Futures adaptor for [`AsyncWriteFrame`]
impl<T> AsyncFrameWrite for T
where
    T: AsyncWrite + Send + 'static,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_close(cx)
    }
}

pub trait AsyncFrameRead: Send + Sync + 'static {
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<Bytes>>;
}

impl<T: Stream<Item = std::io::Result<Bytes>> + Sync + Send + 'static> AsyncFrameRead for T {
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<Bytes>> {
        self.poll_next(cx).map(|x| x.unwrap_or_else(|| Err(std::io::ErrorKind::BrokenPipe.into())))
    }
}
