//! Implementations of RPC codecs ([`Codec`]) for various protocols

#[cfg(feature = "jsonrpc")]
pub mod jsonrpc {
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
            let _ = (method, params, buf);
            Err(crate::codec::error::EncodeError::UnsupportedAction)
        }

        fn encode_request(
            &self,
            request_id: crate::defs::RequestId,
            method: &str,
            params: &dyn erased_serde::Serialize,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), crate::codec::error::EncodeError> {
            let _ = (request_id, method, params, buf);
            Err(crate::codec::error::EncodeError::UnsupportedAction)
        }

        fn encode_response(
            &self,
            request_id_raw: &[u8],
            result: crate::codec::EncodeResponsePayload,
            buf: &mut bytes::BytesMut,
        ) -> Result<(), crate::codec::error::EncodeError> {
            let _ = (request_id_raw, result, buf);
            Err(crate::codec::error::EncodeError::UnsupportedAction)
        }

        fn deserialize_payload(
            &self,
            payload: &[u8],
            visitor: &mut dyn FnMut(
                &mut dyn erased_serde::Deserializer,
            ) -> Result<(), erased_serde::Error>,
        ) -> Result<(), erased_serde::Error> {
            let _ = (payload, visitor);
            let type_name = std::any::type_name::<Self>();
            Err(serde::ser::Error::custom(format!(
                "Codec <{type_name}> does not support argument deserialization"
            )))
        }

        fn codec_noti_hash(&self) -> Option<std::num::NonZeroU64> {
            self.impl_codec_noti_hash()
        }

        fn decode_inbound(
            &self,
            frame: &[u8],
        ) -> Result<crate::codec::InboundFrameType, crate::codec::error::DecodeError> {
            let _ = frame;
            Err(crate::codec::error::DecodeError::UnsupportedAction)
        }
    }
}
#[cfg(feature = "msgpack-rpc")]
pub mod msgpackrpc {}
#[cfg(feature = "mspack-rpc-postcard")]
pub mod msgpackrpc_postcard {}
