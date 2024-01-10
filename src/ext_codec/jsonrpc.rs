use std::ops::Range;

use bytes::BufMut;
use serde::{Deserialize, Serialize};

use crate::{
    codec::{
        self,
        error::{DecodeError, EncodeError},
        CodecUtil, EncodeResponsePayload, InboundFrameType,
    },
    defs::{RequestId, SizeType},
    ResponseError,
};

#[derive(Debug)]
pub struct Codec;

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RawData<'a> {
    Notification {
        jsonrpc: &'a str,
        method: &'a str,
        params: &'a serde_json::value::RawValue,
    },
    Request {
        jsonrpc: &'a str,
        method: &'a str,
        params: &'a serde_json::value::RawValue,
        id: &'a str,
    },
    ResponseSuccess {
        jsonrpc: &'a str,
        result: &'a serde_json::value::RawValue,
        id: &'a str,
    },
    ResponseError {
        jsonrpc: &'a str,
        error: RawErrorObject<'a>,
        id: &'a str,
    },
}

#[derive(serde::Deserialize)]
struct RawErrorObject<'a> {
    code: Option<i64>,
    #[serde(borrow)]
    data: Option<&'a serde_json::value::RawValue>,
    // NOTE: Commenting out `message` field, as we're not using it.
    //
    // message: Option<&'a str>,
}

// ========================================================== Trait Implementation ===|

mod errc {
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const PARSE_ERROR: i64 = -32700;
}

impl crate::Codec for Codec {
    fn encode_notify(
        &self,
        method: &str,
        params: &dyn erased_serde::Serialize,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
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
    ) -> Result<(), EncodeError> {
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
        result: EncodeResponsePayload,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        #[derive(serde::Serialize)]
        struct Encode<'a> {
            jsonrpc: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            result: Option<&'a dyn erased_serde::Serialize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<ErrorEncode<'a>>,
            id: &'a str,
        }

        #[derive(serde::Serialize)]
        struct ErrorEncode<'a> {
            code: i64,
            message: &'a str,
            data: &'a dyn erased_serde::Serialize,
        }

        let request_id =
            std::str::from_utf8(request_id_raw).map_err(|_| EncodeError::NonUtf8StringRequestId)?;

        fn error_to_errc(ec: ResponseError) -> i64 {
            match ec {
                ResponseError::InvalidArgument => errc::INVALID_PARAMS,
                ResponseError::MethodNotFound => errc::METHOD_NOT_FOUND,
                ResponseError::NonUtf8MethodName => errc::INVALID_REQUEST,
                ResponseError::ParseFailed => errc::PARSE_ERROR,
                ResponseError::Unauthorized => errc::INVALID_REQUEST,
                _ => errc::INTERNAL_ERROR,
            }
        }

        let errmsg;
        let result = match result {
            EncodeResponsePayload::Ok(r) => Ok(r),
            EncodeResponsePayload::ErrCodeOnly(ec) => Err(ErrorEncode {
                code: error_to_errc(ec),
                message: {
                    errmsg = ec.to_string();
                    errmsg.as_str()
                },
                data: &errmsg,
            }),
            EncodeResponsePayload::ErrObjectOnly(obj) => Err(ErrorEncode {
                code: errc::INTERNAL_ERROR,
                message: "internal error",
                data: obj,
            }),
            EncodeResponsePayload::Err(ec, obj) => Err(ErrorEncode {
                code: error_to_errc(ec),
                message: {
                    errmsg = ec.to_string();
                    errmsg.as_str()
                },
                data: obj,
            }),
        };

        let encode = match result {
            Ok(ok) => Encode {
                jsonrpc: "2.0",
                result: Some(ok),
                error: None,
                id: request_id,
            },
            Err(err) => Encode {
                jsonrpc: "2.0",
                result: None,
                error: Some(err),
                id: request_id,
            },
        };

        encode
            .serialize(&mut serde_json::Serializer::new(buf.writer()))
            .map_err(<erased_serde::Error as serde::ser::Error>::custom)?;

        Ok(())
    }

    fn deserialize_payload(
        &self,
        payload: &[u8],
        visitor: &mut dyn FnMut(
            &mut dyn erased_serde::Deserializer,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        let mut de = serde_json::Deserializer::from_slice(payload);
        visitor(&mut <dyn erased_serde::Deserializer>::erase(&mut de))?;
        Ok(())
    }

    fn codec_noti_hash(&self) -> Option<std::num::NonZeroU64> {
        self.impl_codec_noti_hash()
    }

    fn decode_inbound(&self, frame: &[u8]) -> Result<codec::InboundFrameType, DecodeError> {
        let mut de = serde_json::Deserializer::from_slice(frame);
        let raw = RawData::deserialize(&mut de)
            .map_err(<erased_serde::Error as serde::de::Error>::custom)?;

        let range_of = |s: &str| -> Range<SizeType> {
            // May panic if the range is out of bounds. (e.g. front of the frame)
            let start = s.as_ptr() as usize - frame.as_ptr() as usize;
            let end = start + s.len();
            start as SizeType..end as SizeType
        };

        let verify_rpc_version = |rpc_version: &str| -> Result<(), DecodeError> {
            if rpc_version != "2.0" {
                Err(DecodeError::UnsupportedProtocol)
            } else {
                Ok(())
            }
        };

        let frame_type = match raw {
            RawData::Request {
                jsonrpc,
                method,
                params,
                id,
            } => {
                verify_rpc_version(jsonrpc)?;
                InboundFrameType::Request {
                    req_id_raw: range_of(id),
                    method: range_of(method),
                    params: range_of(params.get()),
                }
            }
            RawData::Notification {
                jsonrpc,
                method,
                params,
            } => {
                verify_rpc_version(jsonrpc)?;
                InboundFrameType::Notify {
                    method: range_of(method),
                    params: range_of(params.get()),
                }
            }
            RawData::ResponseSuccess {
                jsonrpc,
                result,
                id,
            } => {
                verify_rpc_version(jsonrpc)?;

                InboundFrameType::Response {
                    req_id: req_id_from_str(id)?,
                    errc: None,
                    payload: range_of(result.get()),
                }
            }
            RawData::ResponseError {
                jsonrpc,
                error: RawErrorObject { code, data },
                id,
            } => {
                verify_rpc_version(jsonrpc)?;

                InboundFrameType::Response {
                    req_id: req_id_from_str(id)?,
                    errc: Some(errc_to_error(code.unwrap_or(errc::INTERNAL_ERROR))),
                    payload: data.map(|x| range_of(x.get())).unwrap_or_default(),
                }
            }
        };

        Ok(frame_type)
    }
}

fn errc_to_error(errc: i64) -> ResponseError {
    match errc {
        errc::INVALID_REQUEST => ResponseError::InvalidArgument,
        errc::METHOD_NOT_FOUND => ResponseError::MethodNotFound,
        errc::INVALID_PARAMS => ResponseError::InvalidArgument,
        errc::PARSE_ERROR => ResponseError::ParseFailed,
        _ => ResponseError::Unknown,
    }
}

fn req_id_from_str(s: &str) -> Result<RequestId, DecodeError> {
    Ok(RequestId::new(
        atoi::atoi::<u32>(s.as_bytes())
            .ok_or(DecodeError::RequestIdRetrievalFailed)?
            .try_into()
            .map_err(|_| DecodeError::RequestIdRetrievalFailed)?,
    ))
}
