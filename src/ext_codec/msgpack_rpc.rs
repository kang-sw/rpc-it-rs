use std::{marker::PhantomData, num::NonZeroU64, str::FromStr};

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

/// Partial Implementation of the [msgpack-rpc
/// Specification](https://github.com/msgpack-rpc/msgpack-rpc/blob/master/spec.md)
///
/// This library diverges from the msgpack-rpc specification by not enforcing the wrapping of
/// parameters in an array. Parameters are serialized as they are, without automatic encapsulation.
/// This method may result in a violation of the specification if the user does not encapsulate
/// parameters in a tuple.
///
/// <br>
///
/// ## NOTE: Why Doesn't This Library Automatically Encapsulate Parameters in an Array?
///
/// One might consider the following approach:
///
/// - Serialize the parameter first, then check its marker; if it's not an array, encapsulate it in
///   an array.
///
/// Consider an example where we serialize an `i32` parameter with automatic array encapsulation.
/// During serialization, this would be automatically encapsulated into `[i32]`. Our `proc-macro`
/// feature generates a protocol based on the original type name (e.g., `... { type ParamRecv = i32;
/// ... }`), expecting the original parameter payload to be `i32`.
///
/// Typically, this is generated as a new-typed version of the original value, `struct
/// ParamRecv(i32)`. This might allow for deserialization of a single-argument sequence back into
/// the original value. However, it appears that the `rmp_serde` deserializer does not invoke the
/// `visit_seq` routine during deserialization.
///
/// Could we then 'unwrap' the single-argument serialization back into the original value during
/// decoding? Unfortunately, this is also not feasible. Consider a `Vec<i32>` with a single element.
/// If the decoder naively unwraps single-argument arrays to their inner representation, valid array
/// representations could be mistakenly altered. For instance, `[1]` in a `Vec<i32>` is a valid
/// array representation. If decoded as a single-argument tuple, it would incorrectly unwrap to `1`,
/// resulting in a violation.
///
/// An alternative could be to encapsulate every parameter in a single-argument array, regardless of
/// its content, and then unwrap it during decoding. However, this approach fails when dealing with
/// external remote requirements that expect multiple argument array parameters.
///
/// THEREFORE; this library intentionally does not alter parameter serialization, even if it means
/// not adhering to the msgpack-rpc array type specification. It is the responsibility of the
/// library user to encapsulate parameters into tuples when necessary. To address this within the
/// library, our rpc implementation is permissive regarding parameter types and accepts any type not
/// encapsulated in an array.
#[derive(Default, Clone, Debug)]
pub struct Codec;

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
        (TYPE_ID_NOTIFICATION, method, params)
            .serialize(&mut rmp_serde::Serializer::new(buf.writer()).with_struct_map())
            .map_err(SerDeError::from)
            .map_err(Into::into)
    }

    fn encode_request<S: serde::Serialize>(
        &self,
        mut req_id: crate::defs::RequestId,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<crate::defs::RequestId, EncodeError> {
        // Make the request ID to fit in 32-bit range
        if req_id.get() > u32::MAX as u64 {
            *req_id = (req_id.get() & u32::MAX as u64)
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

        (TYPE_ID_REQUEST, req_id.get() as u32, method, params)
            .serialize(&mut rmp_serde::Serializer::new(buf.writer()).with_struct_map())
            .map_err(SerDeError::from)?;

        Ok(req_id)
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
