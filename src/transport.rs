use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub use bytes::Bytes;
pub use bytes::{Buf, BytesMut};
use futures_util::{AsyncWrite, Stream};

pub struct BufReader<'a> {
    inner: &'a mut BytesMut,
    read_offset: usize,
}

impl AsRef<[u8]> for BufReader<'_> {
    fn as_ref(&self) -> &[u8] {
        self.chunk()
    }
}

impl<'a> Buf for BufReader<'a> {
    fn remaining(&self) -> usize {
        self.inner.len() - self.read_offset
    }

    fn chunk(&self) -> &[u8] {
        &self.inner[self.read_offset..]
    }

    fn advance(&mut self, cnt: usize) {
        self.read_offset += cnt;
        assert!(self.read_offset <= self.inner.len());
    }
}

impl<'a> BufReader<'a> {
    pub fn new(inner: &'a mut BytesMut) -> Self {
        Self { inner, read_offset: 0 }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.chunk()
    }

    pub fn take(&mut self) -> BytesMut {
        self.inner.split_off(self.read_offset)
    }

    pub fn advanced(&self) -> usize {
        self.read_offset
    }

    pub fn is_empty(&self) -> bool {
        self.read_offset == self.inner.len()
    }
}

pub trait AsyncFrameWrite: Send + 'static {
    /// Called before writing a frame. This can be used to deal with writing cancellation.
    fn begin_write_frame(self: Pin<&mut Self>, len: usize) -> std::io::Result<()> {
        let _ = (len,);
        Ok(())
    }

    /// Write a frame to the underlying transport. It can be called multiple times to write a single
    /// frame. In this case, the input buffer is subspan of original frame, which was advanced by
    /// the return byte count of the previous call.
    ///
    /// In this case, it is recommended to indicate buffer advance by return value, not by advancing
    /// the buffer itself. (to reuse the buffer again later).
    ///
    /// Modify buffer(e.g. split off the buffer) when you have to toss the buffer to the channel,
    /// sink, other thread, etc ... Otherwise, treat it just as a read-only buffer.
    ///
    /// # Returns
    ///
    /// Regardless of the buffer is modified or not, the return value should be the number of bytes
    /// written to the underlying transport.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut BufReader,
    ) -> Poll<std::io::Result<()>>;

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
        buf: &mut BufReader,
    ) -> Poll<std::io::Result<()>> {
        match self.poll_write(cx, buf.as_ref())? {
            Poll::Ready(x) => {
                buf.advance(x);
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
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
