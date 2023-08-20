//! # RPC-IT
//!
//! Low-level RPC abstraction
//!
//! # Concepts
//!
//! There are three concepts for RPC handling:
//!
//! - Send Notify
//! - Send Request => Recv Response
//! - Recv Request
//!
//! This library is modeled after the `msgpack-rpc`, but in general, most RPC protocols follow
//! similar patterns, we may adopt this to other protocols in the future. (JSON-RPC, etc...)
//!
//!
//!
//!
//! # Usage
//!
//!
//!

// Re-export crates
pub extern crate bytes;
pub extern crate erased_serde;
pub extern crate futures_util;
pub extern crate serde;

pub mod codec;
pub mod rpc;

pub use rpc::{
    msg::{Notify, RecvMsg, Request, Response},
    Builder, Channel, Client, Feature, InboundError, InboundEventSubscriber, Message, RecvError,
    RequestContext, ResponseFuture, SendError, TryRecvError,
};

pub mod transport {
    pub use bytes::Bytes;
    pub use futures_util::{AsyncRead, AsyncWrite, Stream};

    pub type InboundMessage = std::io::Result<Bytes>;
}

pub mod ext_transport {
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

        impl crate::transport::AsyncWrite for InMemoryWriter {
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

            fn poll_flush(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_close(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
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

        impl crate::transport::Stream for InMemoryReader {
            type Item = crate::transport::InboundMessage;

            fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                let mut inner = self.0.lock();
                if inner.buffer.is_empty() {
                    if inner.writer_dropped {
                        return Poll::Ready(None);
                    }

                    inner.waker.register(cx.waker());
                    Poll::Pending
                } else {
                    Poll::Ready(Some(Ok(inner.chunks.pop_front().unwrap())))
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
}

pub mod ext_codec {
    #[cfg(feature = "msgpack-rpc")]
    mod msgpack_rpc_ {}

    #[cfg(feature = "jsonrpc")]
    mod jsonrpc_ {}
}
