use std::num::NonZeroI32;

use bytes::BytesMut;
use erased_serde::Deserializer as DynDeserializer;
use erased_serde::Serialize as DynSerialize;
use thiserror::Error;

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
