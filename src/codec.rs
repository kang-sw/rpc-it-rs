//! # Codec
//!
//! [`Codec`] is a trait that encodes/decodes data frame into underlying RPC protocol.

use std::{borrow::Cow, ops::Range};

use erased_serde::{Deserializer, Serialize};

/// Splits data stream into frames. For example, for implmenting JSON-RPC over TCP,
/// this would split the stream into JSON-RPC objects delimited by objects.
pub trait Framing: Send + Sync + 'static + Unpin {
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

#[derive(Debug, Clone, Copy)]
pub enum RequestIdType {
    U64,
    Bytes,
    Utf8String,
}

/// Parses/Encodes data frame.
///
/// This is a trait that encodes/decodes data frame into underlying RPC protocol, and generally
/// responsible for any protocol-specific data frame handling.
///
/// The codec, should trivially be clone-able.
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
    /// It should internally generate appropriate request ID, and provide deterministic hash of the
    /// internally generated request ID. This generated ID will be fed to [`Codec::decode_inbound`]
    /// to match the response to the request.
    ///
    /// The generated request ID doesn't need to be deterministic to the req_id_seqn, but only the
    /// returned hash matters.
    ///
    /// # Returns
    ///
    /// Should return for deterministic hash of the (internally generated) request ID.
    ///
    /// This is used to match the response to the request.
    fn encode_request(
        &self,
        method: &str,
        req_id_hint: u64,
        params: &dyn Serialize,
        write: &mut dyn std::io::Write,
    ) -> Result<u64, EncodeError> {
        let _ = (method, req_id_hint, params, write);
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
    /// If `Response` is received, the deterministic hash should be calculated from the request ID.
    ///
    /// # Returns
    ///
    /// Returns the frame type, and the range of the frame.
    fn decode_inbound(&self, data: &[u8]) -> Result<(InboundFrameType, Range<usize>), DecodeError> {
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
        decode: &mut dyn FnMut(&mut dyn Deserializer<'a>) -> Result<(), erased_serde::Error>,
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
