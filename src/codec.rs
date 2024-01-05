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
pub enum ResponseErrorCode {
    /// Possibly in the sender side, the error content didn't originated from this crate, or
    /// [`ResponseErrorCode`] does not define corresponding error code for the underlying protocol.
    /// As the error payload is always preserved, you can parse the payload as generalized
    /// object(e.g. json_value::Value),
    #[default]
    #[error("Unknown error. Parse the payload to acquire more information")]
    Unknown,

    /// Sometimes the RPC protocol itself defines an error code, which is not defined in this crate.
    #[error("Internal error code: {0} (0x{0:08x})")]
    Internal(u32),

    #[error("Server failed to parse the request argument.")]
    InvalidArgument,

    #[error("Unauthorized access to the requested method")]
    Unauthorized,

    #[error("Server is too busy!")]
    Busy,

    #[error("Requested method was not routed")]
    InvalidMethodName,

    #[error("Server explicitly aborted the request")]
    Aborted,

    /// This is a bit special response code that when the request [`crate::Inbound`] is dropped
    /// without any response being sent, this error code will be automatically replied by drop guard
    /// of the request.
    #[error("Server unhandled the request")]
    Unhandled,
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
