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
mod msgpack_rpc {
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
}

#[cfg(feature = "jsonrpc")]
mod jsonrpc {
    use serde::de::DeserializeSeed;
    use serde_json::value::RawValue;

    use crate::codec::{self, ReqId, ReqIdRef};

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
        error: Option<&'a T>,

        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<&'a T>,
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
        jsonrpc: JsonRpcTag,

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
            req_id_hint: u64,
            params: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<std::num::NonZeroU64, codec::EncodeError> {
            serde_json::to_writer(
                write,
                &SerMsg {
                    method: Some(method),
                    id: Some(MsgId::Int(req_id_hint)),
                    params: Some(params),
                    ..Default::default()
                },
            )
            .map_err(|e| codec::EncodeError::SerializeError(e.into()))?;
            Ok(req_id_hint.try_into().unwrap())
        }

        fn encode_response(
            &self,
            req_id: ReqIdRef,
            encode_as_error: bool,
            response: &dyn erased_serde::Serialize,
            write: &mut Vec<u8>,
        ) -> Result<(), codec::EncodeError> {
            let _ = (req_id, response, encode_as_error, write);
            Err(codec::EncodeError::UnsupportedFeature(
                "Response is not supported by this codec".into(),
            ))
        }

        fn encode_response_predefined(
            &self,
            req_id: ReqIdRef,
            response: &codec::PredefinedResponseError,
            write: &mut Vec<u8>,
        ) -> Result<(), codec::EncodeError> {
            self.encode_response(req_id, true, response, write)
        }

        fn decode_inbound(
            &self,
            data: &[u8],
        ) -> Result<(codec::InboundFrameType, std::ops::Range<usize>), codec::DecodeError> {
            let _ = data;
            Err(codec::DecodeError::UnsupportedFeature("This codec is write-only.".into()))
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
