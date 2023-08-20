#[cfg(feature = "tokio")]
mod tokio_ {}

#[cfg(feature = "tokio-tungstenite")]
mod tokio_tungstenite_ {}

#[cfg(feature = "wasm-bindgen-ws")]
mod wasm_websocket_ {}

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

    use bytes::{Bytes, BytesMut};
    use futures_util::task::AtomicWaker;
    use parking_lot::Mutex;

    use crate::transport::{AsyncReadFrame, AsyncWriteFrame};

    struct InMemoryInner {
        buffer: BytesMut,
        chunks: VecDeque<Bytes>,
        waker: AtomicWaker,

        writer_dropped: bool,
        reader_dropped: bool,
    }

    pub struct InMemoryWriter(Arc<Mutex<InMemoryInner>>);
    pub struct InMemoryReader(Arc<Mutex<InMemoryInner>>);

    pub fn new_in_memory() -> (InMemoryWriter, InMemoryReader) {
        let inner = Arc::new(Mutex::new(InMemoryInner {
            buffer: BytesMut::new(),
            chunks: VecDeque::new(),
            waker: AtomicWaker::new(),

            writer_dropped: false,
            reader_dropped: false,
        }));

        (InMemoryWriter(inner.clone()), InMemoryReader(inner.clone()))
    }

    impl AsyncWriteFrame for InMemoryWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut inner = self.0.lock();
            if inner.reader_dropped {
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }

            inner.buffer.extend_from_slice(buf);

            let chunk = inner.buffer.split().freeze();
            inner.chunks.push_back(chunk);

            inner.waker.wake();
            Poll::Ready(Ok(buf.len()))
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

    impl AsyncReadFrame for InMemoryReader {
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<Bytes>> {
            let mut inner = self.0.lock();
            if inner.buffer.is_empty() {
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
