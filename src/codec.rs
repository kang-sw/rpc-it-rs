use bytes::BytesMut;
use erased_serde::Deserializer as DynDeserializer;
use erased_serde::Serialize as DynSerialize;
use serde::Deserialize;
use thiserror::Error;

use self::error::*;
use crate::defs::RequestId;

/// Set of predefined error codes for RPC responses. The codec implementation is responsible for
/// mapping these error codes to the corresponding error codes of the underlying protocol. For
/// example, JSON-RPC uses the `code` field of the response object to encode the error code.
#[derive(Default, Debug, Error, Clone, Copy)]
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

pub enum EncodeResponsePayload<'a> {
    Ok(&'a dyn DynSerialize),
    ErrCodeOnly(ResponseError),
    ErrObjectOnly(&'a dyn DynSerialize),
    Err(ResponseError, &'a dyn DynSerialize),
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

pub trait Codec: std::fmt::Debug + 'static + Send + Sync {
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

    /// This is called from server side, where it responds to the request with received request_id.
    ///
    /// # NOTE
    ///
    /// During encoding request, we can use our internal request id representation as we know how to
    /// deal with 4-byte representation encoding and decoding. However, when we're responding to
    /// received request, we don't know how the request id was encoded as it may not be originated
    /// from this crate's implementation. Therefore, we need to send back the original bytes of
    /// received request id as-is.
    fn encode_response(
        &self,
        request_id_raw: &[u8],
        result: EncodeResponsePayload,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError> {
        let _ = (request_id_raw, result, buf);
        Err(EncodeError::UnsupportedAction)
    }

    fn deserialize_payload(
        &self,
        payload: &[u8],
        visitor: &mut dyn FnMut(&mut dyn DynDeserializer) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        let _ = (payload, visitor);
        let type_name = std::any::type_name::<Self>();
        Err(serde::ser::Error::custom(format!(
            "Codec <{type_name}> does not support argument deserialization"
        )))
    }
}

impl<T> Codec for std::sync::Arc<T>
where
    T: std::fmt::Debug + 'static + Send + Sync + Codec,
{
    fn encode_notify(
        &self,
        method: &str,
        params: &dyn DynSerialize,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError> {
        (**self).encode_notify(method, params, buf)
    }

    fn encode_request(
        &self,
        request_id: RequestId,
        method: &str,
        params: &dyn DynSerialize,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError> {
        (**self).encode_request(request_id, method, params, buf)
    }

    fn encode_response(
        &self,
        request_id_raw: &[u8],
        result: EncodeResponsePayload,
        buf: &mut BytesMut,
    ) -> Result<(), EncodeError> {
        (**self).encode_response(request_id_raw, result, buf)
    }

    fn deserialize_payload(
        &self,
        payload: &[u8],
        visitor: &mut dyn FnMut(&mut dyn DynDeserializer) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        (**self).deserialize_payload(payload, visitor)
    }
}

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum EncodeError {
        #[error("Unsupported type of action")]
        UnsupportedAction,
    }

    #[derive(Debug, Error)]

    pub enum DecodeError {
        #[error("Unsupported type of action")]
        UnsupportedAction,
    }
}

// ========================================================== ParseMessage ===|

/// A generic trait to parse a message into concrete type.
pub trait ParseMessage {
    /// Returns a pair of codec and payload bytes.
    fn codec_payload_pair(&self) -> (&dyn Codec, &[u8]);

    /// This function parses the message into the specified destination.
    ///
    /// Note: This function is not exposed in the public API, mirroring the context of
    /// [`Deserialize::deserialize_in_place`].
    #[doc(hidden)]
    fn parse_in_place<R>(&self, dst: &mut R) -> Result<(), erased_serde::Error>
    where
        R: for<'de> Deserialize<'de>,
    {
        let (codec, buf) = self.codec_payload_pair();
        codec.deserialize_payload(buf.as_ref(), &mut |de| {
            R::deserialize_in_place(de, dst)?;
            Ok(())
        })
    }

    fn parse<R>(&self) -> Result<R, erased_serde::Error>
    where
        R: for<'de> Deserialize<'de>,
    {
        let (codec, buf) = self.codec_payload_pair();
        let mut mem = None;

        codec.deserialize_payload(buf.as_ref(), &mut |de| {
            mem = Some(R::deserialize(de)?);
            Ok(())
        })?;

        if let Some(data) = mem {
            Ok(data)
        } else {
            Err(serde::de::Error::custom(
                "Codec logic error: No data deserialized",
            ))
        }
    }
}
