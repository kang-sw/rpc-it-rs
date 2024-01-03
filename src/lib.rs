pub mod rpc;
pub mod codec {
    use bytes::BytesMut;
    use erased_serde::Deserializer as DynDeserializer;
    use erased_serde::Serialize as DynSerialize;

    pub trait Codec: 'static + Send + Sync {
        fn encode_notify(
            &self,
            method: &str,
            params: &dyn DynSerialize,
            buf: &mut BytesMut,
        ) -> Result<(), EncodeError> {
            let _ = (method, params, buf);
            Err(EncodeError::UnsupportedAction)
        }

        /// Encodes a request message into the given buffer. The `request_id` is contextually unique
        /// integer value, which is used to match the returned response with the request that was
        /// sent. Underlying implementation can encode this `request_id` in any manner or type, as
        /// long as it can be decoded back to the original value.
        fn encode_request(
            &self,
            request_id: RequestId,
            method: &str,
            params: &dyn DynSerialize,
            buf: &mut BytesMut,
        ) -> Result<(), EncodeError> {
            let _ = (request_id, method, params, buf);
            Err(EncodeError::UnsupportedAction)
        }
    }

    pub mod error {
        use thiserror::Error;

        #[derive(Debug, Error)]
        pub enum EncodeError {
            #[error("Unsupported type of action")]
            UnsupportedAction,
        }
    }

    use crate::defs::RequestId;

    use self::error::*;
}
pub mod io {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use bytes::{Buf, Bytes};
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
        fn poll_read_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<Bytes>>;
    }

    // ========================================================== AsyncWriteFrame ===|

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
}
pub mod defs {

    // ========================================================== Basic Types ===|

    use std::ops::Range;

    pub(crate) type SizeType = u32;

    /// 32-bit range type. Defines set of helper methods for working with ranges.
    #[derive(Clone, Copy)]
    pub(crate) struct RangeType([SizeType; 2]);

    // ==== RangeType ====

    impl From<Range<usize>> for RangeType {
        fn from(value: Range<usize>) -> Self {
            Self([value.start as SizeType, value.end as SizeType])
        }
    }

    impl From<RangeType> for Range<usize> {
        fn from(value: RangeType) -> Self {
            Self { start: value.0[0] as usize, end: value.0[1] as usize }
        }
    }

    impl RangeType {
        pub fn new(start: usize, end: usize) -> Self {
            Self([start as SizeType, end as SizeType])
        }

        pub fn r(&self) -> Range<usize> {
            (*self).into()
        }
    }

    // ========================================================== ID Types ===|

    macro_rules! define_id {
		($(#[doc=$doc:literal])* $vis:vis struct $name:ident($base:ty)) => {
			$(#[doc=$doc])*
			#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
			$vis struct $name($base);

            impl $name {
                #[allow(dead_code)]
                pub(crate) fn new(value: $base) -> Self {
                    Self(value)
                }

                pub fn value(&self) -> $base {
                    self.0
                }
            }
		};
	}

    define_id! {
        /// Unique identifier of a RPC request.
        pub struct RequestId(u32)
    }
}

pub mod ext_io {}
pub mod ext_codec {
    //! Implementations of RPC codecs ([`Codec`]) for various protocols

    #[cfg(feature = "jsonrpc")]
    pub mod jsonrpc {}
    #[cfg(feature = "mspack-rpc")]
    pub mod msgpackrpc {}
    #[cfg(feature = "mspack-rpc-postcard")]
    pub mod msgpackrpc_postcard {}
}

// ========================================================== Re-exports ===|

pub extern crate erased_serde;
pub extern crate serde;
