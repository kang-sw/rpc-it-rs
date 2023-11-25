//! # Codec
//!
//! [`Codec`] is a trait that encodes/decodes data frame into underlying RPC protocol.

use std::{
    any::TypeId,
    borrow::Cow,
    num::{NonZeroU64, NonZeroUsize},
    ops::Range,
};

use bytes::BytesMut;
use enum_as_inner::EnumAsInner;
use erased_serde::{Deserializer, Serialize};
use serde::Deserialize;

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
    fn try_framing(&mut self, buffer: &[u8]) -> Result<Option<FramingAdvanceResult>, FramingError>;

    /// Called after every successful frame parsing
    fn advance(&mut self) {}

    /// Returns hint for the next buffer size. This is used to pre-allocate the buffer for the
    /// next [`Self::advance`] call.
    fn next_buffer_size(&self) -> Option<NonZeroUsize> {
        // Default is no. Usually, this is only providable on protocols with frame headers.
        None
    }
}

#[derive(Default, Debug, Clone, Copy)]
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

#[derive(Debug, Clone, EnumAsInner)]
pub enum ReqId {
    U64(u64),
    Bytes(Range<usize>),
    ShortBytes(u8, [u8; 22]),
}

#[derive(Debug, Clone, EnumAsInner)]
pub enum ReqIdRef<'a> {
    U64(u64),
    Bytes(&'a [u8]),
}

impl ReqId {
    pub fn make_ref<'a>(&'a self, buffer: &'a [u8]) -> ReqIdRef<'a> {
        match self {
            ReqId::U64(x) => ReqIdRef::U64(*x),
            ReqId::Bytes(x) => ReqIdRef::Bytes(&buffer[x.clone()]),
            ReqId::ShortBytes(len, buf) => ReqIdRef::Bytes(&buf[..*len as usize]),
        }
    }
}

/// Helper for method [`Codec::as_notification_encoder_hash`].
pub fn encoder_hash_of<T: std::any::Any, H: std::hash::Hash>(
    _this: &T,
    additional_context: &H,
) -> NonZeroU64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    TypeId::of::<T>().hash(&mut hasher);
    additional_context.hash(&mut hasher);

    hasher.finish().try_into().expect("Go and get lottery!")
}

/// Parses/Encodes data frame.
///
/// This is a trait that encodes/decodes data frame into underlying RPC protocol, and generally
/// responsible for any protocol-specific data frame handling.
///
/// The codec, should trivially be clone-able.
pub trait Codec: Send + Sync + 'static + std::fmt::Debug {
    /// Returns the hash of the codec, on encoding notification message. If two different codec
    /// returns same hash, it means that the two codec can be used interchangeably on encoding
    /// notification messages since they'll deterministically produce same output.
    ///
    /// If notification is affected by call order(e.g. not deterministic), this should return None.
    ///
    /// # NOTE
    ///
    /// See [`verify_trait_cast_behavior`] test method implementation.
    fn notification_encoder_hash(&self) -> Option<NonZeroU64> {
        None
    }

    /// Encodes notify frame
    fn encode_notify(
        &self,
        method: &str,
        params: &dyn Serialize,
        write: &mut BytesMut,
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
    /// It is best to the output hash be deterministic for input `req_id_hint`, but it is not
    /// required.
    ///
    /// - `req_id_hint` is guaranteed to be odd number.
    ///
    /// # Returns
    ///
    /// Should return for deterministic hash of the (internally generated) request ID.
    ///
    /// This is used to match the response to the request.
    fn encode_request(
        &self,
        method: &str,
        req_id_hint: NonZeroU64,
        params: &dyn Serialize,
        write: &mut BytesMut,
    ) -> Result<NonZeroU64, EncodeError> {
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
        req_id: ReqIdRef,
        encode_as_error: bool,
        response: &dyn Serialize,
        write: &mut BytesMut,
    ) -> Result<(), EncodeError> {
        let _ = (req_id, response, encode_as_error, write);
        Err(EncodeError::UnsupportedFeature("Response is not supported by this codec".into()))
    }

    /// Encodes predefined response error. See [`PredefinedResponseError`].
    fn encode_response_predefined(
        &self,
        req_id: ReqIdRef,
        response: &PredefinedResponseError,
        write: &mut BytesMut,
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

    /// Tries decode the payload to [`PredefinedResponseError`].
    fn try_decode_predef_error<'a>(&self, payload: &'a [u8]) -> Option<PredefinedResponseError> {
        let _ = (payload,);
        let mut error = None;
        self.decode_payload(payload, &mut |de| {
            let result = PredefinedResponseError::deserialize(de);
            error = Some(result?);
            Ok(())
        })
        .ok()?;
        error
    }
}

impl dyn Codec {
    /// Check if two codecs can be used interchangeably on encoding notification messages.
    pub fn can_reuse_prepared_notification(&self, other: &impl Codec) -> bool {
        self.notification_encoder_hash()
            .zip(other.notification_encoder_hash())
            .is_some_and(|(a, b)| a == b)
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
#[derive(Debug, thiserror::Error, serde::Serialize, serde::Deserialize, EnumAsInner)]
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
    ParseFailed(Cow<'static, str>),

    /// Invalid request/notify handler type
    #[error("Notify handler received request message")]
    NotifyHandler,

    /// Internal error. This is only generated by the user.
    #[error("Internal error: {0}")]
    Internal(i32),

    /// Internal error. This is only generated by the user.
    #[error("Internal error: {0} ({1})")]
    InternalDetailed(i32, String),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EncodeError {
    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(Cow<'static, str>),

    #[error("Unsupported data format: {0}")]
    UnsupportedDataFormat(Cow<'static, str>),

    #[error("Serialization failed: {0}")]
    SerializeError(Box<dyn std::error::Error + Send + Sync + 'static>),

    #[error("Prepared notification mismatch")]
    PreparedNotificationMismatch,

    #[error("Underlying transport does not support reusable notification preparation")]
    PreparedNotificationNotSupported,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(Cow<'static, str>),

    #[error("Unsupported data format: {0}")]
    InvalidFormat(Cow<'static, str>),

    #[error("Parsing error from decoder: {0}")]
    ParseFailed(#[from] erased_serde::Error),

    #[error("Other error reported from decoder: {0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
}

/// Inbound frame type parsed by codec.
#[derive(Debug)]
pub enum InboundFrameType {
    Notify { method: Range<usize> },
    Request { method: Range<usize>, req_id: ReqId },
    Response { req_id: ReqId, req_id_hash: u64, is_error: bool },
}
