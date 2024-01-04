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
#[derive(Debug, Error, Clone, Copy)]
#[non_exhaustive]
pub enum ResponseErrorCode {
    #[error("Custom error. Parse payload to view details.")]
    Custom,

    #[error("Requested method was not routed")]
    MethodNotRouted,

    #[error("Server aborted the request")]
    Aborted,
}

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

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum EncodeError {
        #[error("Unsupported type of action")]
        UnsupportedAction,
    }
}

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
