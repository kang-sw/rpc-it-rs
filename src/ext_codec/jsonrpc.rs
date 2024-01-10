use bytes::BufMut;
use serde::{ser::Error, Serialize};

use crate::codec::CodecUtil;

#[derive(Debug)]
pub struct Codec;

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RawData<'a> {
    Request {
        jsonrpc: &'a str,
        method: &'a str,
        params: &'a serde_json::value::RawValue,
        id: Option<&'a str>,
    },
    Notification {
        jsonrpc: &'a str,
        method: &'a str,
        params: &'a serde_json::value::RawValue,
    },
    ResponseSuccess {
        jsonrpc: &'a str,
        result: &'a serde_json::value::RawValue,
        id: &'a str,
    },
    ResponseError {
        jsonrpc: &'a str,
    },
}

#[derive(serde::Deserialize)]
struct RawErrorObject<'a> {
    code: Option<i64>,
    message: Option<&'a str>,
    data: Option<&'a serde_json::value::RawValue>,
}

// ========================================================== Trait Implementation ===|

impl crate::Codec for Codec {
    fn encode_notify(
        &self,
        method: &str,
        params: &dyn erased_serde::Serialize,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        #[derive(serde::Serialize)]
        struct Encode<'a> {
            jsonrpc: &'static str,
            method: &'a str,
            params: &'a dyn erased_serde::Serialize,
        }

        Encode {
            jsonrpc: "2.0",
            method,
            params,
        }
        .serialize(&mut serde_json::Serializer::new(buf.writer()))
        .map_err(<erased_serde::Error as serde::ser::Error>::custom)?;

        Ok(())
    }

    fn encode_request(
        &self,
        request_id: crate::defs::RequestId,
        method: &str,
        params: &dyn erased_serde::Serialize,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        #[derive(serde::Serialize)]
        struct Encode<'a> {
            jsonrpc: &'static str,
            method: &'a str,
            params: &'a dyn erased_serde::Serialize,
            id: &'a str,
        }

        Encode {
            jsonrpc: "2.0",
            method,
            params,
            id: itoa::Buffer::new().format(request_id.value().get()),
        }
        .serialize(&mut serde_json::Serializer::new(buf.writer()))
        .map_err(<erased_serde::Error as serde::ser::Error>::custom)?;

        Ok(())
    }

    fn encode_response(
        &self,
        request_id_raw: &[u8],
        result: crate::codec::EncodeResponsePayload,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        todo!()
    }

    fn deserialize_payload(
        &self,
        payload: &[u8],
        visitor: &mut dyn FnMut(
            &mut dyn erased_serde::Deserializer,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        todo!()
    }

    fn codec_noti_hash(&self) -> Option<std::num::NonZeroU64> {
        self.impl_codec_noti_hash()
    }

    fn decode_inbound(
        &self,
        frame: &[u8],
    ) -> Result<crate::codec::InboundFrameType, crate::codec::error::DecodeError> {
        todo!()
    }
}
