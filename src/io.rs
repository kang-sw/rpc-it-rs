use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{Future, Sink, SinkExt, Stream};

/// [`AsyncFrameWrite`] is a trait defining the interface for writing data frames
/// to the underlying transport. This trait can either be a straightforward wrapper
/// around the `AsyncWrite` interface or an optimized custom implementation.
/// It may collect [`Bytes`] and flush them in batches to minimize buffer copies.
pub trait AsyncFrameWrite: 'static + Send {
    fn write_frame<'this>(
        self: Pin<&'this mut Self>,
        buf: Bytes,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'this;

    fn write_frame_burst<'this>(
        mut self: Pin<&'this mut Self>,
        bufs: Vec<Bytes>,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'this {
        async move {
            for buf in bufs {
                self.as_mut().write_frame(buf).await?;
            }

            Ok(())
        }
    }

    fn flush<'this>(
        self: Pin<&'this mut Self>,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'this {
        async { Ok(()) }
    }

    fn close<'this>(
        self: Pin<&'this mut Self>,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'this {
        async { Ok(()) }
    }
}

/// [`AsyncFrameRead`] is a trait defining the interface for reading data frames from the
/// underlying transport.
pub trait AsyncFrameRead {
    /// Reads a single frame. The returned frame must be a *complete* message that can be
    /// decoded by a single action. Therefore, the responsibility of framing inbound stream data
    /// lies with the implementation of [`AsyncFrameRead`].
    ///
    /// It'll return [`Ok(None)`] if it was gracefully closed.
    fn poll_read_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<Option<Bytes>>>;
}

// ==== AsyncFrameWrite ====

/// Implements [`AsyncFrameWrite`] for any type that implements [`tokio::io::AsyncWrite`].
impl<T, E> AsyncFrameWrite for T
where
    T: Sink<Bytes, Error = E> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async fn write_frame<'this>(mut self: Pin<&'this mut Self>, buf: Bytes) -> std::io::Result<()> {
        self.send(buf).await.map_err(error_mapping)?;
        Ok(())
    }
}

fn error_mapping(e: impl std::error::Error + Send + Sync + 'static) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}

// ==== AsyncFrameRead ====

impl<T> AsyncFrameRead for T
where
    T: Stream<Item = std::io::Result<Bytes>> + Unpin + Send + 'static,
{
    fn poll_read_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<Option<Bytes>>> {
        Stream::poll_next(self, cx).map(|x| x.transpose())
    }
}

// ========================================================== In-memory ===|

macro_rules! cfg_in_memory_io {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "in-memory-io")]
            $item
        )*
    };
}

cfg_in_memory_io!(
    pub use in_memory::{InMemoryRx, InMemoryTx, in_memory};
);

#[cfg(feature = "in-memory-io")]
mod in_memory {
    use std::{
        num::NonZeroUsize,
        pin::Pin,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
        task::{Context, Poll},
    };

    use bytes::Bytes;
    use futures::{task::AtomicWaker, Stream};

    use crate::{inner::cold_path, AsyncFrameRead};

    /// A naive implementation of in-memory channel
    pub struct InMemoryTx(Arc<InMemoryInner>);

    pub struct InMemoryRx(Arc<InMemoryInner>);

    struct InMemoryInner {
        queue: crossbeam_queue::ArrayQueue<Bytes>,
        semaphore: AtomicUsize,
        sig_msg_sent: AtomicWaker,
        sig_ready_to_recv: AtomicWaker,
        closed: AtomicBool,
    }

    // ========================================================== Impls ===|

    /// Creates pipe IO for in-memory communication.
    pub fn in_memory(capacity: impl TryInto<NonZeroUsize>) -> (InMemoryTx, InMemoryRx) {
        let capacity = capacity
            .try_into()
            .map_err(drop)
            .expect("capacity must be non-zero");

        let inner = Arc::new(InMemoryInner {
            queue: crossbeam_queue::ArrayQueue::new(capacity.get()),
            sig_msg_sent: AtomicWaker::new(),
            sig_ready_to_recv: AtomicWaker::new(),
            closed: Default::default(),
            semaphore: capacity.get().into(),
        });

        (InMemoryTx(inner.clone()), InMemoryRx(inner))
    }

    // ==== InMemoryTx ====

    impl futures::Sink<Bytes> for InMemoryTx {
        type Error = std::io::Error;

        fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            macro_rules! check_ready {
                () => {
                    if self.0.closed.load(Ordering::Relaxed) {
                        return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                    }

                    if self.0.semaphore.load(Ordering::Relaxed) > 0 {
                        return Poll::Ready(Ok(()));
                    }
                };
            }

            check_ready!();

            self.0.sig_ready_to_recv.register(cx.waker());

            check_ready!();

            Poll::Pending
        }

        fn start_send(self: Pin<&mut Self>, mut item: Bytes) -> Result<(), Self::Error> {
            loop {
                // Due to atomic conditions, push ops may fail spuriously. Therefore we spin the
                // expression loop until it succeeds ...
                match self.0.queue.push(item) {
                    Ok(..) => {
                        self.0.semaphore.fetch_sub(1, Ordering::Relaxed);

                        if let Some(w) = self.0.sig_msg_sent.take() {
                            w.wake()
                        }

                        return Ok(());
                    }
                    Err(revoked) => {
                        cold_path();
                        std::thread::yield_now();

                        item = revoked;
                    }
                }
            }
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.drop_impl();
            Poll::Ready(Ok(()))
        }
    }

    impl InMemoryTx {
        fn drop_impl(&self) {
            self.0.closed.store(true, Ordering::Relaxed);

            if let Some(w) = self.0.sig_msg_sent.take() {
                w.wake()
            }
        }
    }

    impl Drop for InMemoryTx {
        fn drop(&mut self) {
            self.drop_impl();
        }
    }

    // ==== InMemoryRx ====

    impl AsyncFrameRead for InMemoryRx {
        fn poll_read_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<Option<Bytes>>> {
            macro_rules! try_recv {
                () => {
                    if let Some(msg) = self.0.queue.pop() {
                        self.0.semaphore.fetch_add(1, Ordering::AcqRel);

                        if let Some(w) = self.0.sig_ready_to_recv.take() {
                            w.wake()
                        }

                        return std::task::Poll::Ready(Ok(Some(msg)));
                    }

                    if self.0.closed.load(Ordering::Relaxed) {
                        return Poll::Ready(Ok(None));
                    }
                };
            }

            try_recv!();

            self.0.sig_msg_sent.register(cx.waker());

            // Try again after registering the waker; maybe we got a message in the meantime.
            try_recv!();

            Poll::Pending
        }
    }

    impl Stream for InMemoryRx {
        type Item = Bytes;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            AsyncFrameRead::poll_read_frame(self, cx).map(|x| x.ok().flatten())
        }
    }

    impl InMemoryRx {
        fn drop_impl(&self) {
            self.0.closed.store(true, Ordering::Relaxed);

            if let Some(w) = self.0.sig_ready_to_recv.take() {
                w.wake()
            }
        }
    }

    impl Drop for InMemoryRx {
        fn drop(&mut self) {
            self.drop_impl();
        }
    }
}
