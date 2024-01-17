use std::ops::Range;

use bytes::BytesMut;
use serde::Deserialize;
use thiserror::Error;

use self::error::*;
use crate::defs::RequestId;
use crate::defs::SizeType;

/// Set of predefined error codes for RPC responses. The codec implementation is responsible for
/// mapping these error codes to the corresponding error codes of the underlying protocol. For
/// example, JSON-RPC uses the `code` field of the response object to encode the error code.
#[derive(Default, Debug, Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseError {
    /// Possibly in the sender side, the error content didn't originated from this crate, or
    /// [`ResponseError`] does not define corresponding error code for the underlying protocol.
    /// As the error payload is always preserved, you can parse the payload as generalized
    /// object(e.g. json_value::Value),
    #[default]
    #[error("Error code couldn't parsed. Parse the payload to acquire more information")]
    Unknown = 0,

    #[error("Server failed to parse the request argument.")]
    InvalidArgument = 1,

    #[error("Unauthorized access to the requested method")]
    Unauthorized = 2,

    #[error("Server is too busy!")]
    Busy = 3,

    #[error("Requested method was not routed")]
    MethodNotFound = 4,

    #[error("Server explicitly aborted the request")]
    Aborted = 5,

    /// This is a bit special response code that when the request [`crate::Inbound`] is dropped
    /// without any response being sent, this error code will be automatically replied by drop guard
    /// of the request.
    #[error("Server unhandled the request")]
    Unhandled = 6,

    #[error("Non UTF-8 Method Name")]
    NonUtf8MethodName = 7,

    #[error("Couldn't parse the request message")]
    ParseFailed = 8,
}

pub enum EncodeResponsePayload<'a, S: serde::Serialize> {
    Ok(&'a S),
    ErrCodeOnly(ResponseError),
    ErrObjectOnly(&'a S),
    Err(ResponseError, &'a S),
}

impl ResponseError {
    /// Unknown is kind of special error code, which is used when the error code is not specified
    /// by the underlying protocol, or the error code is not defined in this crate.
    pub fn is_error_code(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

impl From<u8> for ResponseError {
    fn from(code: u8) -> Self {
        match code {
            0 => Self::Unknown,
            1 => Self::InvalidArgument,
            2 => Self::Unauthorized,
            3 => Self::Busy,
            4 => Self::MethodNotFound,
            5 => Self::Aborted,
            6 => Self::Unhandled,
            7 => Self::NonUtf8MethodName,
            8 => Self::ParseFailed,
            _ => Self::Unknown,
        }
    }
}

impl From<ResponseError> for u8 {
    fn from(code: ResponseError) -> Self {
        match code {
            ResponseError::Unknown => 0,
            ResponseError::InvalidArgument => 1,
            ResponseError::Unauthorized => 2,
            ResponseError::Busy => 3,
            ResponseError::MethodNotFound => 4,
            ResponseError::Aborted => 5,
            ResponseError::Unhandled => 6,
            ResponseError::NonUtf8MethodName => 7,
            ResponseError::ParseFailed => 8,
        }
    }
}

// ========================================================== Codec ===|
#[cfg(feature = "de-error-detail")]
pub type DeserializeError = anyhow::Error;
#[cfg(not(feature = "de-error-detail"))]
pub struct DeserializeError;

#[cfg(not(feature = "de-error-detail"))]
impl<T: std::error::Error> From<T> for DeserializeError {
    fn from(_: T) -> Self {
        DeserializeError
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Decoding payload is not supported for codec {0}")]
pub struct DecodePayloadUnsupportedError(pub &'static str);

pub trait Codec: std::fmt::Debug + 'static + Send + Sync {
    /// Returns the hash value of this codec. If two codec instances return same hash, their
    /// notification result can be safely reused over multiple message transfer. If notification
    /// encoding is stateful(e.g. contextually encrypted), this method should return `None`.
    ///
    /// Deterministic encoding result either means that you can forward received encoded chunk to
    /// other RPC channel directly.
    ///
    /// This method is primarily used for broadcasting notification over multiple rpc channels.
    ///
    /// # Recommended Implementation for Deterministic Codec
    ///
    /// Following code guaranteed to return same hash value for same codec instance, and always
    /// unique for different underlying codec type.
    ///
    /// ```no_run
    /// struct MyCodec;
    ///
    /// impl MyCodec {
    ///     fn codec_type_addr(&self) -> *const () {
    ///         const ADDR: *const () = &();
    ///         ADDR
    ///     }
    /// }
    /// ```
    fn codec_hash_ptr(&self) -> *const () {
        std::ptr::null()
    }

    /// Fork codec instance. This differs from cloning, where it creates a new instance of the
    /// codec, which outputs the same result as the original codec, however, won't duplicate any
    /// internal scratches for encoding/decoding.
    fn fork(&self) -> Self;

    fn encode_notify<S: serde::Serialize>(
        &self,
        method: &str,
        params: &S,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError>;

    /// Encodes a request message into the given buffer. The `request_id` is contextually unique
    /// integer value, which is used to match the returned response with the request that was
    /// sent. Underlying implementation can encode this `request_id` in any manner or type, as
    /// long as it can be decoded back to the original value.
    fn encode_request<S: serde::Serialize>(
        &self,
        request_id: RequestId,
        method: &str,
        params: &S,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError>;

    /// This is called from server side, where it responds to the request with received request_id.
    ///
    /// # NOTE
    ///
    /// During encoding request, we can use our internal request id representation as we know how to
    /// deal with 4-byte representation encoding and decoding. However, when we're responding to
    /// received request, we don't know how the request id was encoded as it may not be originated
    /// from this crate's implementation. Therefore, we need to send back the original bytes of
    /// received request id as-is.
    fn encode_response<S: serde::Serialize>(
        &self,
        request_id_raw: &[u8],
        result: EncodeResponsePayload<S>,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError>;

    /// Create deserializer from the payload part of the original message.
    fn payload_deserializer<'de>(
        &'de self,
        payload: &'de [u8],
    ) -> Result<impl serde::Deserializer<'de>, DecodePayloadUnsupportedError>;

    /// Given frame of the raw bytes, decode it into a message.
    ///
    /// # NOTE
    ///
    /// The frame size is guaranteed to be shorter than 2^32-1 bytes. Therefore, any range in
    /// returned [`InboundFrameType`] should be able to be represented in `u32` type.
    fn decode_inbound(&self, frame: &[u8]) -> Result<InboundFrameType, DecodeError>;
}

/// Describes the inbound frame chunk.
#[derive(Clone)]
pub enum InboundFrameType {
    Request {
        /// The raw request id bytes range.
        req_id_raw: Range<SizeType>,
        method: Range<SizeType>,
        params: Range<SizeType>,
    },
    Notify {
        method: Range<SizeType>,
        params: Range<SizeType>,
    },
    Response {
        req_id: RequestId,

        /// If this value presents, it means that the response is an error response. When response
        /// error code couldn't be parsed but still it's an error, use [`ResponseError::Unknown`]
        /// variant instead.
        errc: Option<ResponseError>,

        /// If response is error, this payload will be the error object's range. If it's the
        /// protocol that can explicitly encode error code, this value can be empty(`[0, 0)`).
        payload: Range<SizeType>,
    },
}

// ==== Codec ====

pub mod error {
    use thiserror::Error;

    use super::DeserializeError;

    #[derive(Debug, Error)]
    pub enum EncodeError {
        #[error("Unsupported type of action")]
        UnsupportedAction,

        #[error("Non UTF-8 string detected: method name")]
        NonUtf8StringMethodName,

        #[error("Non UTF-8 string detected: payload content")]
        NonUtf8StringPayloadContent,

        #[error("Non UTF-8 string detected: request id")]
        NonUtf8StringRequestId,

        #[error("Serialize failed: {0}")]
        SerializeFailed(#[from] DeserializeError),
    }

    #[derive(Debug, Error)]

    pub enum DecodeError {
        #[error("Unsupported type of action")]
        UnsupportedAction,

        #[error("Unsupported protocol")]
        UnsupportedProtocol,

        #[error("Failed to retrieve request ID")]
        RequestIdRetrievalFailed,

        #[error("UTF-8 input is expected")]
        NonUtf8Input,

        #[error("Parse failed: {0}")]
        ParseFailed(#[from] DeserializeError),
    }
}

// ========================================================== ParseMessage ===|

/// A generic trait to parse a message into concrete type.
pub trait ParseMessage<C: Codec> {
    /// Returns a pair of codec and payload bytes.
    #[doc(hidden)]
    fn codec_payload_pair(&self) -> (&C, &[u8]);

    /// This function parses the message into the specified destination.
    ///
    /// Note: This function is not exposed in the public API, mirroring the context of
    /// [`Deserialize::deserialize_in_place`].
    #[doc(hidden)]
    fn parse_in_place<'de, R>(&'de self, dst: &mut R) -> Result<(), DeserializeError>
    where
        R: Deserialize<'de>,
    {
        let (codec, buf) = self.codec_payload_pair();

        R::deserialize_in_place(codec.payload_deserializer(buf.as_ref())?, dst)
            .map_err(err_to_de_error)
    }

    fn parse<'de, R>(&'de self) -> Result<R, DeserializeError>
    where
        R: Deserialize<'de>,
    {
        let (codec, buf) = self.codec_payload_pair();

        R::deserialize(codec.payload_deserializer(buf.as_ref())?).map_err(err_to_de_error)
    }
}

fn err_to_de_error(e: impl std::error::Error) -> DeserializeError {
    #[cfg(feature = "de-error-detail")]
    {
        anyhow::anyhow!("{}", e)
    }
    #[cfg(not(feature = "de-error-detail"))]
    {
        DeserializeError
    }
}

// ========================================================== Dynamic Codec ===|

#[cfg(feature = "dynamic-codec")]
mod dynamic {
    use std::sync::Arc;

    use crate::Codec;

    use super::{DecodePayloadUnsupportedError, EncodeResponsePayload};

    pub trait DynCodec: std::fmt::Debug + Send + Sync {
        fn dynamic_payload_deserializer<'de>(
            &'de self,
            payload: &'de [u8],
        ) -> Result<Box<dyn erased_serde::Deserializer<'de> + 'de>, DecodePayloadUnsupportedError>;

        fn codec_type_addr(&self) -> *const ();

        fn encode_notify(
            &self,
            method: &str,
            params: &dyn erased_serde::Serialize,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError>;

        fn encode_request(
            &self,
            request_id: crate::defs::RequestId,
            method: &str,
            params: &dyn erased_serde::Serialize,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError>;

        fn encode_response(
            &self,
            request_id_raw: &[u8],
            result: EncodeResponsePayload<'_, &dyn erased_serde::Serialize>,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError>;

        fn decode_inbound(
            &self,
            frame: &[u8],
        ) -> Result<super::InboundFrameType, super::DecodeError>;
    }

    impl Codec for Arc<dyn DynCodec> {
        fn payload_deserializer<'de>(
            &'de self,
            payload: &'de [u8],
        ) -> Result<impl serde::Deserializer<'de>, DecodePayloadUnsupportedError> {
            <dyn DynCodec>::dynamic_payload_deserializer(self.as_ref(), payload)
        }

        fn codec_hash_ptr(&self) -> *const () {
            <dyn DynCodec>::codec_type_addr(self.as_ref())
        }

        fn encode_notify<S: serde::Serialize>(
            &self,
            method: &str,
            params: &S,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError> {
            <dyn DynCodec>::encode_notify(self.as_ref(), method, params, buf)
        }

        fn encode_request<S: serde::Serialize>(
            &self,
            request_id: crate::defs::RequestId,
            method: &str,
            params: &S,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError> {
            <dyn DynCodec>::encode_request(self.as_ref(), request_id, method, params, buf)
        }

        fn encode_response<S: serde::Serialize>(
            &self,
            request_id_raw: &[u8],
            result: super::EncodeResponsePayload<S>,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError> {
            type EP<'a, T> = EncodeResponsePayload<'a, T>;

            let ref_body: &dyn erased_serde::Serialize;
            let result: EP<'_, &dyn erased_serde::Serialize> = match result {
                EP::Ok(s) => EP::Ok((ref_body = s, &ref_body).1),
                EP::ErrCodeOnly(s) => EP::ErrCodeOnly(s),
                EP::ErrObjectOnly(s) => EP::ErrObjectOnly((ref_body = s, &ref_body).1),
                EP::Err(e, s) => EP::Err(e, (ref_body = s, &ref_body).1),
            };

            <dyn DynCodec>::encode_response(self.as_ref(), request_id_raw, result, buf)
        }

        fn decode_inbound(
            &self,
            frame: &[u8],
        ) -> Result<super::InboundFrameType, super::DecodeError> {
            <dyn DynCodec>::decode_inbound(self.as_ref(), frame)
        }

        fn fork(&self) -> Self {
            Arc::clone(self)
        }
    }

    impl<T: Codec> DynCodec for T {
        fn dynamic_payload_deserializer<'de>(
            &'de self,
            payload: &'de [u8],
        ) -> Result<Box<dyn erased_serde::Deserializer<'de> + 'de>, DecodePayloadUnsupportedError>
        {
            let boxed = Box::new(<dyn erased_serde::Deserializer<'de>>::erase(
                <Self as Codec>::payload_deserializer(self, payload)?,
            ));

            Ok(boxed)
        }

        fn codec_type_addr(&self) -> *const () {
            <Self as Codec>::codec_hash_ptr(self)
        }

        fn encode_notify(
            &self,
            method: &str,
            params: &dyn erased_serde::Serialize,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError> {
            <Self as Codec>::encode_notify(self, method, &params, buf)
        }

        fn encode_request(
            &self,
            request_id: crate::defs::RequestId,
            method: &str,
            params: &dyn erased_serde::Serialize,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError> {
            <Self as Codec>::encode_request(self, request_id, method, &params, buf)
        }

        fn encode_response(
            &self,
            request_id_raw: &[u8],
            result: EncodeResponsePayload<'_, &dyn erased_serde::Serialize>,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), super::EncodeError> {
            <Self as Codec>::encode_response(self, request_id_raw, result, buf)
        }

        fn decode_inbound(
            &self,
            frame: &[u8],
        ) -> Result<super::InboundFrameType, super::DecodeError> {
            <Self as Codec>::decode_inbound(self, frame)
        }
    }
}

#[cfg(feature = "dynamic-codec")]
pub use dynamic::*;

#[cfg(feature = "dynamic-codec")]
pub type DynamicCodec = std::sync::Arc<dyn dynamic::DynCodec>;
