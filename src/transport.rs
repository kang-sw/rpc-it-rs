use core::{pin::Pin, task::Poll};
use std::task::Context;

use bytes::{Bytes, BytesMut};

/// Asynchronously writes data frame to underlying transport.
///
/// This may be accessed from multiple thread/context. This is cancellation unsafe, as it leaves
/// the stream out in invalid state if the task in cancelled during
/// [`AsyncWriteFrame::poll_write`] is being handled.
pub trait AsyncWriteFrame: Unpin + Send + Sync + 'static {
    fn poll_pre_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let _ = cx;
        Poll::Ready(Ok(()))
    }

    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<(), std::io::Error>>;

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let _ = cx;
        Poll::Ready(Ok(()))
    }
}

/// Asynchronously reads data frame from underlying transport. This doesn't need to be
/// cancellation safe, since cancellation means connection expiration, thus
/// [`AsyncReadFrame::poll_read`] is never called again.
pub trait AsyncReadFrame: Unpin + Send + Sync + 'static {
    /// Polls the underlying transport for frame read.
    ///
    /// Provided buffer should never be modified by the caller. The buffer is solely used for
    /// internal parsing, and the caller should not assume the buffer is in any valid state after
    /// this call.
    ///
    /// Modifying buffer is only valid when this function returns [`Poll::Ready`] with [`Err`]
    ///
    /// # Returns
    ///
    /// - `Poll::Ready(Ok(data))` if single frame is successfully read.
    /// - `Poll::Ready(Err(...))` if any error occurs during read.
    /// - `Poll::Pending` if the underlying transport is not ready.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut BytesMut,
    ) -> Poll<Result<Bytes, std::io::Error>>;

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let _ = cx;
        Poll::Ready(Ok(()))
    }
}

/// Asynchronously reads data stream from underlying transport.
///
/// This should be combined with [`crate::codec::Framing`] to split the stream into frames.
/// This is shameless conceptual copy of `AsyncRead` trait from `tokio`/`futures` crate.
pub trait AsyncRead: Unpin + Send + Sync + 'static {
    /// Polls the underlying transport for data read.
    ///
    /// # Returns
    ///
    /// - `Poll::Ready(Ok(bytes_read))` if data is successfully read.
    /// - `Poll::Ready(Err(...))` if any error occurs during read.
    /// - `Poll::Pending` if the underlying transport is not ready.
    ///
    /// # Warning
    ///
    /// This should be cancellation safe.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<Result<usize, std::io::Error>>;

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let _ = cx;
        Poll::Ready(Ok(()))
    }
}
