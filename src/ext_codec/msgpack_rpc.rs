//! Partial implementation of [msgpack-rpc
//! specification](https://github.com/msgpack-rpc/msgpack-rpc/blob/master/spec.md)

use std::{io::Write, marker::PhantomData, num::NonZeroU64};

use arrayvec::ArrayVec;
use bytes::{Buf, BufMut};
use rmp::Marker;
use serde::Serialize;

use crate::{
    codec::{
        error::{DecodeError, EncodeError},
        AsDeserializer, SerDeError,
    },
    defs::RequestId,
};

#[derive(Clone, Debug)]
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
        encode_noti_or_req(method, params, None, buf).map(drop)
    }

    fn encode_request<S: serde::Serialize>(
        &self,
        request_id: crate::defs::RequestId,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<crate::defs::RequestId, EncodeError> {
        encode_noti_or_req(method, params, Some(request_id), buf).map(Option::unwrap)
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
            ERP::Ok(ok) => (1, req_id, (), ok).serialize(ser),

            // When there's error code, it is serialized as (code, errobj) tuple. No error code,
            // just errobj. If failed to parse error code, the entire slot is treated as errobj.
            ERP::ErrCodeOnly(ec) => (1, req_id, (ec.to_str(), ()), ()).serialize(ser),
            ERP::ErrObjectOnly(obj) => (1, req_id, obj, ()).serialize(ser),
            ERP::Err(ec, obj) => (1, req_id, (ec.to_str(), obj), ()).serialize(ser),
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
        scratch: &mut bytes::BytesMut,
        frame: &mut bytes::Bytes,
    ) -> Result<crate::codec::InboundFrameType, crate::codec::error::DecodeError> {
        todo!()
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
        [Marker::FixMap(4), Marker::FixPos(0)].map(|x| x.to_u8())
    } else {
        [Marker::FixMap(3), Marker::FixPos(2)].map(|x| x.to_u8())
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
    let is_array_parameter = rmp::decode::read_array_len(&mut &buf[data_start_pos..]).is_ok();

    // Head position changes whether it's array or not
    if is_array_parameter {
        buf.advance(1);
    } else {
        buf[data_start_pos - 1] = rmp::Marker::FixArray(1).to_u8();
    };

    // Finish by writing header and method name.
    buf[..head.len()].copy_from_slice(head);
    buf[head.len()..data_start_pos - 1].copy_from_slice(method.as_bytes());

    Ok(request_id)
}
