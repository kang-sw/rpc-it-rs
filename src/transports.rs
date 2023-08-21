#[cfg(feature = "tokio")]
mod tokio_io {
    use std::{pin::Pin, task::Poll};

    use bytes::Buf;
    use tokio::io::ReadBuf;

    use crate::transport::FrameReader;

    pub struct TokioWriteFrameWrapper<T> {
        inner: T,
    }

    pub trait ToWriteFrame<T> {
        fn to_write_frame(self) -> TokioWriteFrameWrapper<T>;
    }

    impl<T> ToWriteFrame<T> for T
    where
        T: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        fn to_write_frame(self) -> TokioWriteFrameWrapper<T> {
            TokioWriteFrameWrapper { inner: self }
        }
    }

    impl<T> crate::transport::AsyncFrameWrite for TokioWriteFrameWrapper<T>
    where
        T: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut FrameReader,
        ) -> std::task::Poll<std::io::Result<()>> {
            if let Poll::Ready(n) = Pin::new(&mut self.inner).poll_write(cx, buf.as_slice())? {
                buf.advance(n);
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        }

        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_close(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    pub struct TokioAsyncReadWrapper<T> {
        inner: T,
    }

    pub trait ToAsyncRead<T> {
        fn to_async_read(self) -> TokioAsyncReadWrapper<T>;
    }

    impl<T> ToAsyncRead<T> for T
    where
        T: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        fn to_async_read(self) -> TokioAsyncReadWrapper<T> {
            TokioAsyncReadWrapper { inner: self }
        }
    }

    impl<T> futures_util::AsyncRead for TokioAsyncReadWrapper<T>
    where
        T: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let mut buf = ReadBuf::new(buf);
            if Pin::new(&mut self.inner).poll_read(cx, &mut buf)?.is_pending() {
                Poll::Pending
            } else {
                Poll::Ready(Ok(buf.filled().len()))
            }
        }
    }
}

#[cfg(feature = "tokio-tungstenite")]
mod tokio_tungstenite_ {
    // TODO: Implement I/O on splitted streams
}

#[cfg(feature = "wasm-bindgen-ws")]
mod wasm_websocket_ {
    // TODO: Implement I/O on splitted streams
}

#[cfg(feature = "in-memory")]
pub use in_memory_::*;
#[cfg(feature = "in-memory")]
mod in_memory_ {
    use std::{
        collections::VecDeque,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll},
    };

    use bytes::Bytes;
    use futures_util::task::AtomicWaker;
    use parking_lot::Mutex;

    use crate::transport::{AsyncFrameRead, AsyncFrameWrite, FrameReader};

    struct InMemoryInner {
        chunks: VecDeque<Bytes>,
        waker: AtomicWaker,

        writer_dropped: bool,
        reader_dropped: bool,
    }

    pub struct InMemoryWriter(Arc<Mutex<InMemoryInner>>);
    pub struct InMemoryReader(Arc<Mutex<InMemoryInner>>);

    pub fn new_in_memory() -> (InMemoryWriter, InMemoryReader) {
        let inner = Arc::new(Mutex::new(InMemoryInner {
            chunks: VecDeque::new(),
            waker: AtomicWaker::new(),

            writer_dropped: false,
            reader_dropped: false,
        }));

        (InMemoryWriter(inner.clone()), InMemoryReader(inner.clone()))
    }

    impl AsyncFrameWrite for InMemoryWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut FrameReader,
        ) -> Poll<std::io::Result<()>> {
            let mut inner = self.0.lock();
            if inner.reader_dropped {
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }

            inner.chunks.push_back(buf.take().freeze());
            inner.waker.wake();
            Poll::Ready(Ok(()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            let mut inner = self.0.lock();
            inner.writer_dropped = true;
            Poll::Ready(Ok(()))
        }
    }

    impl Drop for InMemoryWriter {
        fn drop(&mut self) {
            let mut inner = self.0.lock();
            inner.writer_dropped = true;
            inner.waker.wake();
        }
    }

    impl AsyncFrameRead for InMemoryReader {
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<Bytes>> {
            let mut inner = self.0.lock();
            if inner.chunks.is_empty() {
                if inner.writer_dropped {
                    return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                }

                inner.waker.register(cx.waker());
                Poll::Pending
            } else {
                Poll::Ready(Ok(inner.chunks.pop_front().unwrap()))
            }
        }
    }

    impl Drop for InMemoryReader {
        fn drop(&mut self) {
            let mut inner = self.0.lock();
            inner.reader_dropped = true;
        }
    }
}
