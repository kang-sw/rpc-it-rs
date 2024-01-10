use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{Sink, Stream};

/// [`AsyncFrameWrite`] is a trait defining the interface for writing data frames
/// to the underlying transport. This trait can either be a straightforward wrapper
/// around the `AsyncWrite` interface or an optimized custom implementation.
/// It may collect [`Bytes`] and flush them in batches to minimize buffer copies.
pub trait AsyncFrameWrite: 'static + Send {
    /// Notifies the underlying transport about a new frame.
    fn start_frame(self: Pin<&mut Self>) -> std::io::Result<()> {
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
    fn poll_write_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut Bytes,
    ) -> Poll<std::io::Result<()>> {
        futures::ready!(self.as_mut().poll_ready(cx)).map_err(error_mapping)?;
        Poll::Ready(self.start_send(buf.split_off(0)).map_err(error_mapping))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Sink::poll_flush(self, cx).map_err(error_mapping)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Sink::poll_close(self, cx).map_err(error_mapping)
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
            macro_rules! check {
                () => {
                    if self.0.closed.load(Ordering::Relaxed) {
                        return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                    }

                    if self.0.semaphore.load(Ordering::Relaxed) > 0 {
                        return Poll::Ready(Ok(()));
                    }
                };
            }

            check!();

            self.0.sig_ready_to_recv.register(cx.waker());

            check!();

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
