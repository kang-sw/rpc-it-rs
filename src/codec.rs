//! # Codec
//!
//! [`Codec`] is a trait that encodes/decodes data frame into underlying RPC protocol.

use std::{borrow::Cow, num::NonZeroUsize, ops::Range};

use erased_serde::{Deserializer, Serialize};

/// Splits data stream into frames. For example, for implmenting JSON-RPC over TCP,
/// this would split the stream into JSON-RPC objects delimited by objects.
pub trait Framing: Send + Sync + 'static + Unpin {
    /// Advance internal parsing status
    ///
    /// # Returns
    ///
    /// - `Ok(Some(Range))` if a frame is found. Returns the range, represents `(valid_data_end,
    ///   next_frame_start)` respectively.
    /// - `Ok(None)` if a frame is not found. The buffer should be kept as-is, and the next
    ///   [`Self::advance`] call should be called with the same buffer, but extended with more data
    ///   from the underlying transport.
    /// - `Err(...)` if any error occurs during framing.
    fn advance(&mut self, buffer: &[u8]) -> Result<Option<FramingAdvanceResult>, FramingError>;

    /// Returns hint for the next buffer size. This is used to pre-allocate the buffer for the
    /// next [`Self::advance`] call.
    fn next_buffer_size(&self) -> Option<NonZeroUsize>;
}

#[derive(Debug, Clone, Copy)]
pub struct FramingAdvanceResult {
    pub valid_data_end: usize,
    pub next_frame_start: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum FramingError {
    #[error("Broken buffer. The connection should be closed. Context: {0}")]
    Broken(Cow<'static, str>),

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
pub trait Codec: Send + Sync + 'static + std::fmt::Debug {
    /// Encodes notify frame
    fn encode_notify(
        &self,
        method: &str,
        params: &dyn Serialize,
        write: &mut Vec<u8>,
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
        write: &mut Vec<u8>,
    ) -> Result<u64, EncodeError> {
        let _ = (method, req_id_hint, params, write);
        Err(EncodeError::UnsupportedFeature("Request is not supported by this codec".into()))
    }

    /// Encodes response frame
    ///
    /// - `req_id`: The original request ID.
    /// - `encode_as_error`: If true, the response should be encoded as error response.
    /// - `response`: The response object.
    /// - `write`: The writer to write the response to.
    fn encode_response(
        &self,
        req_id: &[u8],
        encode_as_error: bool,
        response: &dyn Serialize,
        write: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        let _ = (req_id, response, encode_as_error, write);
        Err(EncodeError::UnsupportedFeature("Response is not supported by this codec".into()))
    }

    /// Encodes predefined response error. See [`PredefinedResponseError`].
    fn encode_response_predefined(
        &self,
        req_id: &[u8],
        response: &PredefinedResponseError,
        write: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        self.encode_response(req_id, true, response, write)
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

/// Some predefined reponse error types. Most of them are automatically generated from the server.
///
/// In default, errors are serialized as following form:
///
/// ```json
/// {
///     "code": "PARSE_FAILED",
///     "detail": "Failed to parse argument. (hint: Type 'i32' is expected)"
/// }
/// ```
///
/// The encoded error types may be customized by [`Codec::encode_response_predefined`].
#[derive(Debug, thiserror::Error, serde::Serialize)]
#[non_exhaustive]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "code", content = "detail")]
pub enum PredefinedResponseError {
    /// You should not use this error type directly. This error type is used internally by the
    /// library when the request object is dropped before sending request. To specify intentional
    /// abort, use [`PredefinedResponseError::Aborted`] instead.
    #[error("Request object was dropped by server.")]
    Unhandled,

    /// Use this when you want to abort the request intentionally.
    #[error("Request was intentionally aborted")]
    Aborted,

    /// The typename will be obtained from [`std::any::type_name`]
    #[error("Failed to parse argument. (hint: Type '{0}' is expected)")]
    ParseFailed(&'static str),

    /// Internal error. This is only generated by the user.
    #[error("Internal error: {0}")]
    Internal(i32),

    /// Internal error. This is only generated by the user.
    #[error("Internal error: {0} ({1})")]
    InternalDetailed(i32, String),
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
    Response { req_id: Range<usize>, req_id_hash: u64, is_error: bool },
}
