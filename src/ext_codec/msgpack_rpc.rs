//! Partial implementation of [msgpack-rpc
//! specification](https://github.com/msgpack-rpc/msgpack-rpc/blob/master/spec.md)

use std::{marker::PhantomData, num::NonZeroU64, str::FromStr};

use arrayvec::ArrayVec;
use bytes::{Buf, BufMut};
use rmp::Marker;
use serde::Serialize;

use crate::{
    codec::{
        error::{DecodeError, EncodeError},
        AsDeserializer, InboundFrameType, SerDeError,
    },
    defs::RequestId,
    ResponseError,
};

const TYPE_ID_REQUEST: u8 = 0;
const TYPE_ID_RESPONSE: u8 = 1;
const TYPE_ID_NOTIFICATION: u8 = 2;

#[derive(Clone, Debug)]
pub struct Codec {
    /// Determines whether the codec validates the format of the codec parameter. For instance,
    /// if the input parameter is not enclosed in an array, this option enables the codec to
    /// automatically wrap it in an array.
    ///
    /// If disabled, the codec serializes the parameter as-is, even if it's invalid. Such
    /// parameters will likely be rejected by a correctly implemented server, as this
    /// implementation does not refuse unformatted parameters.
    ///
    /// Utilizing this library with procedural macros typically warrants enabling this option
    /// (which is the default behavior). However, if you're facilitating communication within your
    /// own internal system without procedural macros, enabling this may lead to deserialization
    /// errors. This is because the serialization type might not be compatible with the
    /// deserialization type, unless you are deserializing into Serde's automatically generated
    /// new type wrappers.
    enc_validate_param: bool,
}

impl Default for Codec {
    fn default() -> Self {
        Self {
            enc_validate_param: true,
        }
    }
}

impl Codec {
    pub fn with_encoding_parameter_validation(mut self, validate: bool) -> Self {
        self.enc_validate_param = validate;
        self
    }
}

impl crate::Codec for Codec {
    fn codec_reusability_id(&self) -> usize {
        static ADDR: () = ();
        &ADDR as *const () as usize
    }

    fn encode_notify<S: serde::Serialize>(
        &self,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        encode_noti_or_req(method, params, None, buf, self.enc_validate_param).map(drop)
    }

    fn encode_request<S: serde::Serialize>(
        &self,
        request_id: crate::defs::RequestId,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<crate::defs::RequestId, EncodeError> {
        encode_noti_or_req(
            method,
            params,
            Some(request_id),
            buf,
            self.enc_validate_param,
        )
        .map(Option::unwrap)
    }

    fn encode_response<S: serde::Serialize>(
        &self,
        raw_req_id: &[u8],
        result: crate::codec::EncodeResponsePayload<S>,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        // We should be able to read request ID as u32 from raw bytes. Validate it.
        let req_id = retrieve_req_id(raw_req_id).ok_or(EncodeError::InvalidRequestId)?;

        use crate::codec::EncodeResponsePayload as ERP;
        let ser = &mut rmp_serde::Serializer::new(buf.writer()).with_struct_map();

        match result {
            ERP::Ok(ok) => (TYPE_ID_RESPONSE, req_id, (), ok).serialize(ser),

            // When there's error code, it is serialized as (code, errobj) tuple. No error code,
            // just errobj. If failed to parse error code, the entire slot is treated as errobj.
            ERP::ErrCodeOnly(ec) => {
                (TYPE_ID_RESPONSE, req_id, (ec.to_str(), ()), ()).serialize(ser)
            }

            ERP::ErrObjectOnly(obj) => (TYPE_ID_RESPONSE, req_id, obj, ()).serialize(ser),
            ERP::Err(ec, obj) => (TYPE_ID_RESPONSE, req_id, (ec.to_str(), obj), ()).serialize(ser),
        }
        .map_err(SerDeError::from)
        .map_err(Into::into)
        .map(drop)
    }

    fn payload_deserializer<'de>(
        &'de self,
        payload: &'de [u8],
    ) -> Result<impl crate::codec::AsDeserializer<'de>, crate::codec::DecodePayloadUnsupportedError>
    {
        struct Wrapper<'de, R: 'de>(
            rmp_serde::Deserializer<R, rmp_serde::config::DefaultConfig>,
            PhantomData<&'de ()>,
        );

        impl<'de, R> AsDeserializer<'de> for Wrapper<'de, R>
        where
            R: rmp_serde::decode::ReadSlice<'de>,
        {
            fn as_deserializer(&mut self) -> impl serde::Deserializer<'de> {
                &mut self.0
            }

            fn is_human_readable(&self) -> bool {
                false
            }
        }

        Ok(Wrapper(
            rmp_serde::Deserializer::from_read_ref(payload),
            PhantomData,
        ))
    }

    fn decode_inbound(
        &self,
        _scratch: &mut bytes::BytesMut,
        frame: &mut bytes::Bytes,
    ) -> Result<InboundFrameType, crate::codec::error::DecodeError> {
        // We don't need mutable reference.
        let frame = &frame[..];
        let rd = &mut { frame };

        use rmp::decode as dec;

        fn pos(base: &[u8], cursor: &[u8]) -> u32 {
            (cursor.as_ptr() as usize - base.as_ptr() as usize) as u32
        }

        // *Poor man's try block*
        (|| {
            let array_size = dec::read_array_len(rd)?;
            let type_id: u8 = dec::read_int(rd)?;

            let frame_type = match (array_size, type_id) {
                (4, ty @ TYPE_ID_REQUEST) | (3, ty @ TYPE_ID_NOTIFICATION) => {
                    let req_id_range = if ty == TYPE_ID_REQUEST {
                        let req_id_start_pos = pos(frame, rd);
                        let _: u32 = dec::read_int(rd)?;

                        Some(req_id_start_pos..pos(frame, rd))
                    } else {
                        None
                    };

                    let method_len: u32 = dec::read_str_len(rd)?;
                    let method_start_pos = pos(frame, rd);

                    rd.advance(method_len as _);
                    let method = method_start_pos..pos(frame, rd);

                    let params_start_pos = pos(frame, rd);
                    let params = params_start_pos..frame.len() as u32;

                    if let Some(req_id_raw) = req_id_range {
                        InboundFrameType::Request {
                            req_id_raw,
                            method,
                            params,
                        }
                    } else {
                        InboundFrameType::Notify { method, params }
                    }
                }
                (4, TYPE_ID_RESPONSE) => {
                    let req_id: u32 = dec::read_int(rd)?;
                    let req_id = RequestId::new((req_id as u64).try_into()?);

                    let is_ok_result = if dec::read_nil(&mut { *rd }).is_ok() {
                        dec::read_nil(rd).unwrap();
                        true
                    } else {
                        if *frame.last().unwrap() != Marker::Null.to_u8() {
                            // Check if result slot is correctly specifying `nil` marker
                            return Err(DecodeError::InvalidFormat.into());
                        }

                        false
                    };

                    // If it's error, the last byte is error object's `nil` marker
                    let payload_end = frame.len() as u32 - (!is_ok_result as u32);
                    let payload_start = pos(frame, rd);

                    let payload = payload_start..payload_end;

                    // Here, cursor is pointing to either error object or returned result object.
                    if is_ok_result {
                        InboundFrameType::Response {
                            req_id,
                            errc: None,
                            payload,
                        }
                    } else {
                        // error is `obj` or `(errc:&str, obj: any)`.

                        // Try to parse the error object as `(errc, obj)`
                        let errc_payload = 'errc_find: {
                            let rd = &mut { *rd };

                            if !dec::read_array_len(rd).is_ok_and(|x| x == 2) {
                                break 'errc_find None;
                            }

                            let Ok((err_str, tail)) = dec::read_str_from_slice(&*rd) else {
                                break 'errc_find None;
                            };

                            let Ok(errc) = ResponseError::from_str(err_str) else {
                                break 'errc_find None;
                            };

                            // Rest of the data is error object part.
                            let payload = pos(frame, tail)..payload_end;
                            Some((errc, payload))
                        };

                        if let Some((errc, payload)) = errc_payload {
                            InboundFrameType::Response {
                                req_id,
                                errc: Some(errc),
                                payload,
                            }
                        } else {
                            InboundFrameType::Response {
                                req_id,
                                errc: Some(Default::default()),
                                payload, // Treat the whole object as error object.
                            }
                        }
                    }
                }
                _ => return Err(DecodeError::InvalidFormat.into()),
            };

            Ok::<_, SerDeError>(frame_type)
        })()
        .map_err(Into::into)
    }

    fn restore_request_id(
        &self,
        raw_id: &[u8],
    ) -> Result<crate::defs::RequestId, crate::codec::error::DecodeError> {
        Ok(RequestId::new(
            retrieve_req_id(raw_id)
                .map(|x| x as u64)
                .ok_or(DecodeError::InvalidFormat)?
                .try_into()
                .map_err(|_| DecodeError::InvalidFormat)?,
        ))
    }
}

fn retrieve_req_id(raw: &[u8]) -> Option<u32> {
    rmp::decode::read_u32(&mut { raw }).ok()
}

fn encode_noti_or_req<S: serde::Serialize>(
    method: &str,
    params: &S,
    request_id: Option<RequestId>,
    buf: &mut bytes::BytesMut,
    validate_param_format: bool,
) -> Result<Option<RequestId>, EncodeError> {
    debug_assert!(buf.is_empty());

    // Allocates one more byte than required, and start serialization of `params` firstly ...
    // This is to wrap the single `params` object into an array, as required by the spec. This
    // works by inserting single array byte ahead of `params` serialization, and then writing
    // rest of the data in front of the params byte.

    // Fisrt we calculate reserved buffer size for method and params serialization
    use rmp::encode as enc;

    let mut request_id = request_id;
    let is_request = request_id.is_some();

    // At any case, it can't exceed 16 bytes
    // - FixMap, 1B
    // - TypeID, 1B
    // - (optional) Request ID, Max 5B (1B marker, 4B payload)
    // - Method name length, Max 5B (1B marker, 4B payload)
    let head = &mut ArrayVec::<u8, 12>::new();

    let init_marker = if is_request {
        [Marker::FixArray(4), Marker::FixPos(TYPE_ID_REQUEST)].map(|x| x.to_u8())
    } else {
        [Marker::FixArray(3), Marker::FixPos(TYPE_ID_NOTIFICATION)].map(|x| x.to_u8())
    };

    head.extend(init_marker);

    // For request, `msgid` slot is addtionally reserved.
    if is_request {
        // Make the request ID to fit in 32-bit range
        let req_id = request_id.as_mut().unwrap();
        if req_id.get() > u32::MAX as u64 {
            **req_id = (req_id.get() & u32::MAX as u64)
                .try_into()
                .unwrap_or_else(|_| {
                    // It's cold: only 0 will be 1. Which is single case out of 2^32.
                    crate::cold_path();

                    // SAFETY: 1 is not zero.
                    unsafe { NonZeroU64::new_unchecked(1) }
                });
        } else {
            // In most cases, the original request ID will exceed 32-bit range as it was generated
            // randomly at first.
            crate::cold_path();
        }

        let req_id = req_id.get() as u32;
        enc::write_u32(head, req_id).unwrap();
    }

    enc::write_str_len(head, method.len() as _).expect("data exceeded 32-bit range");

    // We'll write method name later. For now, we just reserve space for it.
    let data_start_pos = head.len() + method.len() + 1 /* as margin */;

    // Just reserve few more bytes for really required amount.
    const DATA_MARGIN: usize = 32;
    buf.reserve(DATA_MARGIN + data_start_pos);

    // SAFETY: We just reserved enough space for `head` and `method` serialization.
    unsafe { buf.set_len(data_start_pos) }

    // Encode params at buffer position
    params
        .serialize(&mut rmp_serde::Serializer::new(buf.writer()).with_struct_map())
        .map_err(SerDeError::from)?;

    // Check if the serialized value is array or not
    let is_array_parameter = || rmp::decode::read_array_len(&mut &buf[data_start_pos..]).is_ok();

    // Head position changes whether it's array or not
    if !validate_param_format || is_array_parameter() {
        buf.advance(1);
    } else {
        buf[data_start_pos - 1] = rmp::Marker::FixArray(1).to_u8();
    }

    // Finish by writing header and method name.
    buf[..head.len()].copy_from_slice(head);
    buf[head.len()..head.len() + method.len()].copy_from_slice(method.as_bytes());

    Ok(request_id)
}
