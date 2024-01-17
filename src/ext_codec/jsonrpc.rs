use std::ops::Range;

use bytes::BufMut;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue as RawJsonValue;

use crate::{
    codec::{
        self,
        error::{DecodeError, EncodeError},
        DeserializeError, EncodeResponsePayload, InboundFrameType,
    },
    defs::{RequestId, SizeType},
    ResponseError,
};

#[derive(Debug, Clone, Copy)]
pub struct Codec;

#[derive(Debug, serde::Deserialize)]
struct RawData<'a> {
    jsonrpc: &'a str,

    #[serde(borrow, default)]
    method: Option<&'a str>,

    #[serde(borrow, default)]
    params: Option<&'a RawJsonValue>,

    #[serde(borrow, default)]
    result: Option<&'a RawJsonValue>,

    #[serde(default)]
    error: Option<RawErrorObject<'a>>,

    #[serde(borrow, default)]
    id: Option<&'a str>,
}

#[derive(Debug, serde::Deserialize)]
struct RawErrorObject<'a> {
    code: Option<i64>,
    #[serde(borrow)]
    data: Option<&'a RawJsonValue>,
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

    /// Offset for internal error code.
    pub const INTERNAL_ERROR_OFFSET: i64 = -41000;
}

impl crate::Codec for Codec {
    fn codec_hash_ptr(&self) -> *const () {
        const ADDR: *const () = &();
        ADDR
    }

    fn fork(&self) -> Self {
        Self
    }

    fn encode_notify<S: Serialize>(
        &self,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        #[derive(serde::Serialize)]
        struct Encode<'a, S: Serialize> {
            jsonrpc: &'static str,
            method: &'a str,
            params: &'a S,
        }

        Encode {
            jsonrpc: "2.0",
            method,
            params,
        }
        .serialize(&mut serde_json::Serializer::new(buf.writer()))
        .map_err(DeserializeError::from)?;

        Ok(())
    }

    fn encode_request<S: Serialize>(
        &self,
        request_id: crate::defs::RequestId,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        #[derive(serde::Serialize)]
        struct Encode<'a, S: Serialize> {
            jsonrpc: &'static str,
            method: &'a str,
            params: &'a S,
            id: &'a str,
        }

        Encode {
            jsonrpc: "2.0",
            method,
            params,
            id: itoa::Buffer::new().format(request_id.value().get()),
        }
        .serialize(&mut serde_json::Serializer::new(buf.writer()))
        .map_err(DeserializeError::from)?;

        Ok(())
    }

    fn encode_response<S: Serialize>(
        &self,
        request_id_raw: &[u8],
        result: EncodeResponsePayload<S>,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        #[derive(serde::Serialize)]
        struct Encode<'a, S: Serialize> {
            jsonrpc: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            result: Option<&'a S>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<ErrorEncode<'a, S>>,
            id: &'a str,
        }

        #[derive(serde::Serialize)]
        struct ErrorEncode<'a, S: Serialize> {
            code: i64,
            message: &'a str,
            data: ErrorEncodeData<'a, S>,
        }

        #[derive(serde::Serialize)]
        #[serde(untagged)]
        enum ErrorEncodeData<'a, S: Serialize> {
            Msg(&'a str),
            Obj(&'a S),
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
                ec => u8::from(ec) as i64 + errc::INTERNAL_ERROR_OFFSET,
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
                data: ErrorEncodeData::Msg(&errmsg),
            }),
            EncodeResponsePayload::ErrObjectOnly(obj) => Err(ErrorEncode {
                code: errc::INTERNAL_ERROR,
                message: "internal error",
                data: ErrorEncodeData::Obj(obj),
            }),
            EncodeResponsePayload::Err(ec, obj) => Err(ErrorEncode {
                code: error_to_errc(ec),
                message: {
                    errmsg = ec.to_string();
                    errmsg.as_str()
                },
                data: ErrorEncodeData::Obj(obj),
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
            .map_err(DeserializeError::from)?;

        Ok(())
    }

    fn decode_inbound(&self, frame: &[u8]) -> Result<codec::InboundFrameType, DecodeError> {
        let frame = std::str::from_utf8(frame).map_err(|_| DecodeError::NonUtf8Input)?;
        let mut de = serde_json::Deserializer::from_str(frame);
        let raw = RawData::deserialize(&mut de).map_err(DeserializeError::from)?;

        let range_of = |s: &str| -> Range<SizeType> {
            // May panic if the range is out of bounds. (e.g. front of the frame)
            let start = s.as_ptr() as usize - frame.as_ptr() as usize;
            let end = start + s.len();
            start as SizeType..end as SizeType
        };

        if raw.jsonrpc != "2.0" {
            return Err(DecodeError::UnsupportedProtocol);
        }

        #[derive(thiserror::Error, Debug)]
        #[error("Invalid JSON-RPC frame received")]
        struct InvalidJsonRpcFrame;

        let frame_type = match raw {
            RawData {
                method: Some(method),
                params: Some(params),
                id: Some(id),
                ..
            } => InboundFrameType::Request {
                req_id_raw: range_of(id),
                method: range_of(method),
                params: range_of(params.get()),
            },
            RawData {
                method: Some(method),
                params: Some(params),
                ..
            } => InboundFrameType::Notify {
                method: range_of(method),
                params: range_of(params.get()),
            },
            RawData {
                result: Some(result),
                id: Some(id),
                ..
            } => InboundFrameType::Response {
                req_id: req_id_from_str(id)?,
                errc: None,
                payload: range_of(result.get()),
            },
            RawData {
                error: Some(RawErrorObject { code, data }),
                id: Some(id),
                ..
            } => InboundFrameType::Response {
                req_id: req_id_from_str(id)?,
                errc: Some(errc_to_error(code.unwrap_or(errc::INTERNAL_ERROR))),
                payload: data.map(|x| range_of(x.get())).unwrap_or_default(),
            },
            _other => {
                return Err(DeserializeError::from(InvalidJsonRpcFrame).into());
            }
        };

        Ok(frame_type)
    }

    fn payload_deserializer<'de>(
        &'de self,
        payload: &'de [u8],
    ) -> Result<impl serde::Deserializer<'de>, codec::DecodePayloadUnsupportedError> {
        Ok(DeserializeWrapper(serde_json::Deserializer::from_slice(
            payload,
        )))
    }
}

fn errc_to_error(errc: i64) -> ResponseError {
    match errc {
        errc::INVALID_REQUEST => ResponseError::InvalidArgument,
        errc::METHOD_NOT_FOUND => ResponseError::MethodNotFound,
        errc::INVALID_PARAMS => ResponseError::InvalidArgument,
        errc::PARSE_ERROR => ResponseError::ParseFailed,
        other => ResponseError::from(
            u8::try_from(other - errc::INTERNAL_ERROR_OFFSET).unwrap_or_default(),
        ),
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

struct DeserializeWrapper<T>(serde_json::Deserializer<T>);

macro_rules! inherit {
    ($($name:ident),*) => {
        $(
            fn $name<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>
            where
                V: serde::de::Visitor<'de>,
            {
                (&mut self.0).$name(visitor)
            }
        )*
    };
}

impl<'de, T> serde::Deserializer<'de> for DeserializeWrapper<T>
where
    T: serde_json::de::Read<'de>,
{
    type Error = serde_json::Error;

    inherit!(
        deserialize_any,
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_option,
        deserialize_unit,
        deserialize_seq,
        deserialize_map,
        deserialize_identifier,
        deserialize_ignored_any
    );

    fn deserialize_unit_struct<V>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        (&mut { self.0 }).deserialize_unit_struct(name, visitor)
    }

    fn deserialize_newtype_struct<V>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        (&mut { self.0 }).deserialize_newtype_struct(name, visitor)
    }

    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        (&mut { self.0 }).deserialize_tuple(len, visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        (&mut { self.0 }).deserialize_tuple_struct(name, len, visitor)
    }

    fn deserialize_struct<V>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        (&mut { self.0 }).deserialize_struct(name, fields, visitor)
    }

    fn deserialize_enum<V>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        (&mut { self.0 }).deserialize_enum(name, variants, visitor)
    }
}
