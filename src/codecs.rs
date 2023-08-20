pub mod framing {
    use memchr::memmem;

    use crate::codec::{self, Framing};

    /// Splits a buffer into frames by byte sequence delimeter
    #[cfg(feature = "delim-framing")]
    #[derive(Debug)]
    struct DelimeterFraming {
        finder: memmem::Finder<'static>,
        cursor: usize,
    }

    #[cfg(feature = "delim-framing")]
    impl Framing for DelimeterFraming {
        fn try_framing(
            &mut self,
            buffer: &[u8],
        ) -> Result<Option<codec::FramingAdvanceResult>, codec::FramingError> {
            let buf = &buffer[self.cursor..];
            if let Some(pos) = self.finder.find(buf) {
                let valid_data_end = self.cursor + pos;
                let next_frame_start = valid_data_end + self.finder.needle().len();
                self.cursor = next_frame_start;
                Ok(Some(codec::FramingAdvanceResult { valid_data_end, next_frame_start }))
            } else {
                // Remain some margin to not miss the delimeter
                self.cursor += buffer.len().saturating_sub(self.finder.needle().len());
                Ok(None)
            }
        }

        fn advance(&mut self) {
            self.cursor = 0;
        }

        fn next_buffer_size(&self) -> Option<std::num::NonZeroUsize> {
            None
        }
    }

    #[cfg(feature = "delim-framing")]
    pub fn by_delim(delim: &[u8]) -> impl Framing {
        DelimeterFraming { cursor: 0, finder: memmem::Finder::new(delim).into_owned() }
    }

    /// TODO: Implment framing by root object/array
    #[cfg(feature = "json-framing")]
    #[derive(Debug)]
    struct JsonFraming {}
}

#[cfg(feature = "msgpack-rpc")]
pub mod msgpack_rpc {
    use std::{borrow::Cow, num::NonZeroU64};

    use derive_setters::Setters;

    use crate::codec::{self, DecodeError::InvalidFormat, EncodeError};

    #[derive(Setters, Debug, Default)]
    pub struct Codec {
        /// If specified, the codec will wrap the provided parameter to array automatically.
        /// Otherwise, the caller should wrap the parameter within array manually.
        ///
        /// > [Reference](https://github.com/msgpack-rpc/msgpack-rpc/blob/master/spec.md#params)
        auto_param_array_wrap: bool,

        /// On deseiralization, if the parameter is an array with single element, the codec will
        /// unwrap the array and use the element as the parameter. Otherwise, the parameter will be
        /// deserialized as an array.
        unwrap_mono_param_array: bool,
    }

    impl codec::Codec for Codec {
        fn encode_notify(
            &self,
            method: &str,
            params: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<(), EncodeError> {
            use rmp::encode::*;
            write_array_len(write, 3).unwrap();
            write_uint(write, 2).unwrap();
            write_str(write, method).unwrap();

            if self.auto_param_array_wrap {
                write_array_len(write, 1).unwrap();
            }

            params
                .erased_serialize(&mut <dyn erased_serde::Serializer>::erase(
                    &mut rmp_serde::Serializer::new(write).with_human_readable(),
                ))
                .unwrap();

            Ok(())
        }

        fn encode_request(
            &self,
            method: &str,
            req_id_hint: NonZeroU64,
            params: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<std::num::NonZeroU64, EncodeError> {
            use rmp::encode::*;
            write_array_len(write, 4).unwrap();
            write_uint(write, 0).unwrap();

            let as_32bit = req_id_hint.get() as u32;
            write_u32(write, as_32bit).unwrap();
            write_str(write, method).unwrap();

            if self.auto_param_array_wrap {
                write_array_len(write, 1).unwrap();
            }

            params
                .erased_serialize(&mut <dyn erased_serde::Serializer>::erase(
                    &mut rmp_serde::Serializer::new(write).with_human_readable(),
                ))
                .unwrap();

            Ok((as_32bit as u64).try_into().unwrap())
        }

        fn encode_response(
            &self,
            req_id: codec::ReqIdRef,
            encode_as_error: bool,
            response: &dyn erased_serde::Serialize,
            mut write: &mut Vec<u8>,
        ) -> Result<(), EncodeError> {
            use rmp::encode::*;
            write_array_len(write, 4).unwrap();

            write_uint(write, 1).unwrap();
            write_u32(
                write,
                *req_id
                    .as_u64()
                    .ok_or(EncodeError::UnsupportedDataFormat("unsupported non-integer".into()))?
                    as _,
            )
            .unwrap();

            let serialize = |v: &mut Vec<u8>| {
                response
                    .erased_serialize(&mut <dyn erased_serde::Serializer>::erase(
                        &mut rmp_serde::Serializer::new(v).with_human_readable(),
                    ))
                    .unwrap();
            };

            if encode_as_error {
                serialize(write);
                write_nil(write).unwrap();
            } else {
                write_nil(write).unwrap();
                serialize(write);
            }

            Ok(())
        }

        fn encode_response_predefined(
            &self,
            req_id: codec::ReqIdRef,
            response: &codec::PredefinedResponseError,
            write: &mut Vec<u8>,
        ) -> Result<(), EncodeError> {
            self.encode_response(req_id, true, response, write)
        }

        fn decode_inbound(
            &self,
            data: &[u8],
        ) -> Result<(codec::InboundFrameType, std::ops::Range<usize>), codec::DecodeError> {
            use rmp::decode::*;
            let mut data_read = data;
            let rd = &mut data_read;

            fn efmt<T>(e: impl Into<Cow<'static, str>>) -> impl FnOnce(T) -> codec::DecodeError {
                |_| InvalidFormat(e.into())
            }

            let arr_len = read_array_len(rd).map_err(efmt("Non-msgpack array format"))?;
            if arr_len < 2 || arr_len > 4 {
                return Err(InvalidFormat(format!("Invalid array length {arr_len}").into()));
            }

            let msg_type: u32 = read_int(rd).map_err(efmt("Non-msgpack integer format"))?;
            match (arr_len, msg_type) {
                // Request
                (4, 0) => {
                    let req_id: u32 =
                        read_int(rd).map_err(efmt("req_id: Non-msgpack integer format"))?;
                    let method = read_str_from_slice(rd);
                    // TODO: wip ..
                }

                // Response
                (4, 1) => {}

                // Notify
                (3, 2) => {}

                (al, msg) => {
                    return Err(InvalidFormat(
                        format!("Invalid message type {msg}, with {al} args").into(),
                    ));
                }
            }

            todo!()
        }

        fn decode_payload<'a>(
            &self,
            payload: &'a [u8],
            decode: &mut dyn FnMut(
                &mut dyn erased_serde::Deserializer<'a>,
            ) -> Result<(), erased_serde::Error>,
        ) -> Result<(), codec::DecodeError> {
            let _ = (payload, decode);
            Err(codec::DecodeError::UnsupportedFeature("This codec is write-only.".into()))
        }
    }
}

#[cfg(feature = "jsonrpc")]
pub mod jsonrpc {
    use std::num::NonZeroU64;

    use serde_json::value::RawValue;

    use crate::codec::{self, InboundFrameType, ReqId, ReqIdRef};

    #[derive(Debug, Default)]
    pub struct Codec {}

    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(untagged)]
    enum MsgId<'a> {
        Int(u64),
        Str(&'a str),
        Null,
    }

    #[derive(serde::Serialize)]
    struct SerMsg<'a, T: serde::Serialize + ?Sized> {
        jsonrpc: JsonRpcTag,

        #[serde(skip_serializing_if = "Option::is_none")]
        method: Option<&'a str>,

        #[serde(rename = "id", skip_serializing_if = "Option::is_none")]
        id: Option<MsgId<'a>>,

        #[serde(skip_serializing_if = "Option::is_none")]
        params: Option<&'a T>,

        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<SerErrObj<'a, T>>,

        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<&'a T>,
    }

    #[derive(serde::Serialize)]
    struct SerErrObj<'a, T: serde::Serialize + ?Sized> {
        code: i64,
        message: &'a str,
        data: &'a T,
    }

    impl<'a, T: serde::Serialize + ?Sized> Default for SerMsg<'a, T> {
        fn default() -> Self {
            Self {
                jsonrpc: Default::default(),
                method: Default::default(),
                id: Default::default(),
                params: Default::default(),
                error: Default::default(),
                result: Default::default(),
            }
        }
    }

    #[derive(Default)]
    struct JsonRpcTag;

    impl serde::Serialize for JsonRpcTag {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer.serialize_str("2.0")
        }
    }

    impl<'de> serde::Deserialize<'de> for JsonRpcTag {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            <&str>::deserialize(deserializer).and_then(|x| {
                if x == "2.0" {
                    Ok(JsonRpcTag)
                } else {
                    Err(serde::de::Error::custom("Invalid JSON-RPC version"))
                }
            })
        }
    }

    #[derive(Default, serde::Deserialize)]
    struct DeMsgFrame<'a> {
        #[serde(rename = "jsonrpc")]
        _jsonrpc: JsonRpcTag,

        #[serde(borrow, default)]
        method: Option<&'a str>,

        #[serde(borrow, default)]
        id: Option<MsgId<'a>>,

        #[serde(borrow, default)]
        params: Option<&'a RawValue>,

        #[serde(borrow, default)]
        error: Option<&'a RawValue>,

        #[serde(borrow, default)]
        result: Option<&'a RawValue>,
    }

    impl codec::Codec for Codec {
        fn encode_notify(
            &self,
            method: &str,
            params: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<(), codec::EncodeError> {
            serde_json::to_writer(
                write,
                &SerMsg { method: Some(method), params: Some(params), ..Default::default() },
            )
            .map_err(|e| codec::EncodeError::SerializeError(e.into()))?;
            Ok(())
        }

        fn encode_request(
            &self,
            method: &str,
            req_id_hint: NonZeroU64,
            params: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<std::num::NonZeroU64, codec::EncodeError> {
            serde_json::to_writer(
                write,
                &SerMsg {
                    method: Some(method),
                    id: Some(MsgId::Int(req_id_hint.get())),
                    params: Some(params),
                    ..Default::default()
                },
            )
            .map_err(|e| codec::EncodeError::SerializeError(e.into()))?;
            Ok(req_id_hint)
        }

        fn encode_response(
            &self,
            req_id: ReqIdRef,
            encode_as_error: bool,
            response: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<(), codec::EncodeError> {
            serde_json::to_writer(
                write,
                &SerMsg {
                    id: Some(match req_id {
                        ReqIdRef::U64(value) => MsgId::Int(value),
                        ReqIdRef::Bytes(value) => {
                            std::str::from_utf8(value).map_or(MsgId::Null, MsgId::Str)
                        }
                    }),
                    error: {
                        (encode_as_error == true).then_some(SerErrObj {
                            code: -1,
                            message: "Error from 'rpc_it::codecs::jsonrpc'",
                            data: response,
                        })
                    },
                    result: (encode_as_error == false).then_some(response),
                    ..Default::default()
                },
            )
            .map_err(|e| codec::EncodeError::SerializeError(e.into()))?;
            Ok(())
        }

        fn encode_response_predefined(
            &self,
            req_id: ReqIdRef,
            response: &codec::PredefinedResponseError,
            write: &mut Vec<u8>,
        ) -> Result<(), codec::EncodeError> {
            // TODO: New type for predefined response error?
            self.encode_response(req_id, true, response, write)
        }

        fn decode_inbound(
            &self,
            data: &[u8],
        ) -> Result<(InboundFrameType, std::ops::Range<usize>), codec::DecodeError> {
            let f = serde_json::from_slice::<DeMsgFrame>(data)
                .map_err(|e| codec::DecodeError::Other(e.into()))?;

            let data_range_of = |x: &[u8]| {
                let offset = x.as_ptr() as usize - data.as_ptr() as usize;
                offset..offset + x.len()
            };

            let method_range = f.method.map(|x| data_range_of(x.as_bytes()));
            let req_id = match &f.id {
                Some(MsgId::Int(x)) => Some(ReqId::U64(*x)),
                Some(MsgId::Str(x)) => Some(ReqId::Bytes(data_range_of(x.as_bytes()))),
                Some(MsgId::Null) => {
                    return Err(codec::DecodeError::InvalidFormat(
                        "Null request ID returned".into(),
                    ))
                }
                None => None,
            };

            Ok(match (&f.id, f.method, f.params, f.error, f.result) {
                // ID, Method, (Params) => Request
                (Some(_id), Some(_), payload, None, None) => (
                    InboundFrameType::Request {
                        method: method_range.unwrap(),
                        req_id: req_id.unwrap(),
                    },
                    payload.map(|value| data_range_of(value.get().as_bytes())).unwrap_or(0..0),
                ),

                // Method, (Params) => Notify
                (None, Some(_), payload, None, None) => (
                    InboundFrameType::Notify { method: method_range.unwrap() },
                    payload.map(|value| data_range_of(value.get().as_bytes())).unwrap_or(0..0),
                ),

                // ID, (Error | Result) => Response
                (Some(_id), None, None, e, r) if e.is_some() ^ r.is_some() => {
                    let MsgId::Int(req_id) = f.id.unwrap() else {
						return Err(codec::DecodeError::InvalidFormat(
							"We don't use string request ID types.".into(),
						))
					};

                    (
                        InboundFrameType::Response {
                            req_id: ReqId::U64(req_id),
                            req_id_hash: req_id,
                            is_error: e.is_some(),
                        },
                        data_range_of(e.or(r).unwrap().get().as_bytes()),
                    )
                }

                _ => {
                    return Err(codec::DecodeError::InvalidFormat(
                        format!(
                            "Invalid json-rpc with fields: \
							 [id?={},method?={},params?={},error?={},result?={}]",
                            f.id.is_some(),
                            f.method.is_some(),
                            f.params.is_some(),
                            f.error.is_some(),
                            f.result.is_some()
                        )
                        .into(),
                    ))
                }
            })
        }

        fn decode_payload<'a>(
            &self,
            payload: &'a [u8],
            decode: &mut dyn FnMut(
                &mut dyn erased_serde::Deserializer<'a>,
            ) -> Result<(), erased_serde::Error>,
        ) -> Result<(), codec::DecodeError> {
            decode(&mut <dyn erased_serde::Deserializer>::erase(
                &mut serde_json::Deserializer::from_slice(payload),
            ))?;
            Ok(())
        }
    }
}
