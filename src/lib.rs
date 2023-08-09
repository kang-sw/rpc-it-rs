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

pub extern crate erased_serde;
/// Re-exported crates
pub extern crate serde;

pub mod transport {
    use core::{pin::Pin, task::Poll};
    use std::task::Context;

    /// Asynchronously writes data frame to underlying transport.
    ///
    /// This may be accessed from multiple thread/context. This is cancellation unsafe, as it leaves
    /// the stream out in invalid state if the task in cancelled during
    /// [`AsyncWriteFrame::poll_write`] is being handled.
    pub trait AsyncWriteFrame: Send + Sync + 'static {
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
    }

    /// Asynchronously reads data frame from underlying transport. This doesn't need to be
    /// cancellation safe, since cancellation means connection expiration, thus
    /// [`AsyncReadFrame::poll_read`] is never called again.
    pub trait AsyncReadFrame: Send + 'static {
        /// Polls the underlying transport for frame read.
        ///
        /// # Returns
        ///
        /// - `Poll::Ready(Ok(frame_length))` if single frame is successfully read.
        /// - `Poll::Ready(Err(...))` if any error occurs during read.
        /// - `Poll::Pending` if the underlying transport is not ready.
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            write: &mut dyn std::io::Write,
        ) -> Poll<Result<usize, std::io::Error>>;
    }

    /// Asynchronously reads data stream from underlying transport.
    ///
    /// This should be combined with [`crate::codec::Framing`] to split the stream into frames.
    /// This is shameless conceptual copy of `AsyncRead` trait from `tokio`/`futures` crate.
    pub trait AsyncRead: Send + 'static {
        /// Polls the underlying transport for data read.
        ///
        /// # Returns
        ///
        /// - `Poll::Ready(Ok(bytes_read))` if data is successfully read.
        /// - `Poll::Ready(Err(...))` if any error occurs during read.
        /// - `Poll::Pending` if the underlying transport is not ready.
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buffer: &mut [u8],
        ) -> Poll<Result<usize, std::io::Error>>;
    }
}

pub mod codec {
    //! # Codec
    //!
    //! [`Codec`] is a trait that encodes/decodes data frame into underlying RPC protocol.

    use std::{borrow::Cow, ops::Range};

    use erased_serde::{Deserializer, Serialize};

    /// Splits data stream into frames. For example, for implmenting JSON-RPC over TCP,
    /// this would split the stream into JSON-RPC objects delimited by objects.
    pub trait Framing {
        /// Advance internal parsing status
        ///
        /// # Returns
        ///
        /// - `Ok(Some(Range))` if a frame is found. The range is the range of the frame from the
        ///   beginning of the input buffer. After returning valid range, from the next
        ///   [`Self::advance`] call the buffer should be sliced from the end of the range.
        /// - `Ok(None)` if a frame is not found. The buffer should be kept as-is, and the next
        ///   [`Self::advance`] call should be called with the same buffer, but extended with more
        ///   data from the underlying transport.
        /// - `Err(...)` if any error occurs during framing.
        fn advance(&mut self, buffer: &[u8]) -> Result<Option<Range<usize>>, FramingError>;
    }

    #[derive(Debug, thiserror::Error)]
    pub enum FramingError {
        #[error("Broken buffer. The connection should be closed. Context: {0}")]
        BrokenBuffer(Cow<'static, str>),

        #[error("Error occurred, but internal state can be restored after {0} bytes")]
        Recoverable(usize),
    }

    /// Parses/Encodes data frame.
    ///
    /// This is a trait that encodes/decodes data frame into underlying RPC protocol, and generally
    /// responsible for any protocol-specific data frame handling.
    pub trait Codec: Send + Sync + 'static {
        /// Encodes notify frame
        fn encode_notify(
            &self,
            method: &str,
            params: &dyn Serialize,
            write: &mut dyn std::io::Write,
        ) -> Result<(), EncodeError> {
            let _ = (method, params, write);
            Err(EncodeError::UnsupportedFeature("Notify is not supported by this codec".into()))
        }

        /// Encodes request frame
        ///
        /// # Returns
        ///
        /// Should return for deterministic hash of the request ID.
        ///
        /// This is used to match the response to the request.
        fn encode_request(
            &self,
            method: &str,
            req_id: &[u8],
            params: &dyn Serialize,
            write: &mut dyn std::io::Write,
        ) -> Result<u64, EncodeError> {
            let _ = (method, req_id, params, write);
            Err(EncodeError::UnsupportedFeature("Request is not supported by this codec".into()))
        }

        /// Encodes response frame
        fn encode_response(
            &self,
            req_id: &[u8],
            is_error: bool,
            response: &dyn Serialize,
        ) -> Result<(), EncodeError> {
            let _ = (req_id, response, is_error);
            Err(EncodeError::UnsupportedFeature("Response is not supported by this codec".into()))
        }

        /// Decodes inbound frame, and identifies the frame type.
        ///
        /// # Returns
        ///
        /// Returns the frame type, and the range of the frame.
        fn decode_inbound(
            &self,
            data: &[u8],
        ) -> Result<(InboundFrameType, Range<usize>), DecodeError> {
            let _ = data;
            Err(DecodeError::UnsupportedFeature("This codec is write-only.".into()))
        }

        /// Decodes the payload of the inbound frame.
        ///
        /// Codec implementation should call `decode` with created [`Deserializer`] object.
        /// Its type information can be erased using `<dyn erased_serde::Deserializer>::erase`
        fn decode_payload<'a>(
            &self,
            payload: &'a [u8],
            decode: &mut dyn FnMut(&mut dyn Deserializer<'a>) -> erased_serde::Error,
        ) -> Result<(), DecodeError> {
            let _ = (payload, decode);
            Err(DecodeError::UnsupportedFeature("This codec is write-only.".into()))
        }
    }

    #[derive(Debug, thiserror::Error)]
    pub enum EncodeError {
        #[error("Unsupported feature: {0}")]
        UnsupportedFeature(Cow<'static, str>),

        #[error("Unsupported data format: {0}")]
        UnsupportedDataFormat(Cow<'static, str>),
    }

    #[derive(Debug, thiserror::Error)]
    pub enum DecodeError {
        #[error("Unsupported feature: {0}")]
        UnsupportedFeature(Cow<'static, str>),

        #[error("Unsupported data format: {0}")]
        UnsupportedDataFormat(Cow<'static, str>),

        #[error("Parsing error from decoder: {0}")]
        ParseFailed(#[from] erased_serde::Error),
    }

    /// Inbound frame type parsed by codec.
    #[derive(Debug)]
    pub enum InboundFrameType {
        Notify { method: Range<usize> },
        Request { method: Range<usize>, req_id: Range<usize> },
        Response { req_id_hash: u64, is_error: bool },
    }
}

pub mod rpc {
    /// Creates RPC connection from [`crate::transport::AsyncReadFrame`] and
    /// [`crate::transport::AsyncWriteFrame`], and [`crate::codec::Codec`].
    ///
    /// For unsupported features(e.g. notify from client), the codec should return
    /// [`crate::codec::EncodeError::UnsupportedFeature`] error.
    pub trait Rpc {}

    pub trait RpcExt {}
}

pub mod prelude {
    pub use crate::transport::{AsyncReadFrame, AsyncWriteFrame};
}
