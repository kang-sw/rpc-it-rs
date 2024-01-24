use std::{mem::take, num::NonZeroU64};

use bytes::BytesMut;
use postcard::{ser_flavors::ExtendFlavor, serialize_with_flavor};
use serde::Deserialize;

use crate::{
    codec::{
        error::{DecodeError, EncodeError},
        AsDeserializer, InboundFrameType, SerDeError,
    },
    defs::RequestId,
    ResponseError,
};

/// This is postcard-based private data exchange protocol.
#[derive(Clone, Debug)]
pub struct Codec;

/// Raw-rpc serialization is simple; it's just adjacent pair of postcard-encoded head and body.
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Head<'a> {
    /// (Method)
    Notify(&'a str),

    /// (Request ID, Method)
    Request(&'a [u8], &'a str),

    /// (Request ID)
    Response(&'a [u8]),

    /// (Request ID, Error Code)
    ///
    /// This does not involve error object; i.e. the payload is empty.
    ResponseErr(&'a [u8], u8 /* errc */),
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
        serialize(buf, &Head::Notify(method))?;
        serialize(buf, params)?;

        Ok(())
    }

    fn encode_request<S: serde::Serialize>(
        &self,
        request_id: RequestId,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<RequestId, EncodeError> {
        serialize(buf, &Head::Request(request_id.as_bytes(), method))?;
        serialize(buf, params)?;

        Ok(request_id)
    }

    fn encode_response<S: serde::Serialize>(
        &self,
        request_id_raw: &[u8],
        result: crate::codec::EncodeResponsePayload<S>,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), EncodeError> {
        let request_id = self
            .restore_request_id(request_id_raw)
            .map_err(|_| EncodeError::InvalidRequestId)?;
        let req_bytes = request_id.as_bytes();

        use crate::codec::EncodeResponsePayload as ERP;
        let (errc, errobj) = match result {
            ERP::Ok(r) => {
                serialize(buf, &Head::Response(req_bytes))?;
                serialize(buf, &r)?;
                return Ok(());
            }

            ERP::ErrCodeOnly(ec) => (ec, None),
            ERP::ErrObjectOnly(eo) => (ResponseError::Unknown, Some(eo)),
            ERP::Err(ec, eo) => (ec, Some(eo)),
        };

        // Convert to u8 error code
        let errc = errc as _;

        if let Some(errobj) = errobj {
            serialize(buf, &Head::ResponseErr(req_bytes, errc))?;
            serialize(buf, &errobj)?;
        } else {
            serialize(buf, &Head::ResponseErr(req_bytes, errc))?;
            serialize(buf, &errc)?; // Payload is just error code.
        }

        Ok(())
    }

    fn payload_deserializer<'de>(
        &'de self,
        payload: &'de [u8],
    ) -> Result<impl AsDeserializer<'de>, crate::codec::DecodePayloadUnsupportedError> {
        struct Wrapper<'de>(postcard::Deserializer<'de, postcard::de_flavors::Slice<'de>>);

        impl<'de> AsDeserializer<'de> for Wrapper<'de> {
            fn as_deserializer(&mut self) -> impl serde::Deserializer<'de> {
                &mut self.0
            }

            fn is_human_readable(&self) -> bool {
                false
            }
        }

        Ok(Wrapper(postcard::Deserializer::from_bytes(payload)))
    }

    fn decode_inbound(
        &self,
        _scratch: &mut bytes::BytesMut,
        frame: &mut bytes::Bytes,
    ) -> Result<crate::codec::InboundFrameType, crate::codec::error::DecodeError> {
        // Parse head
        let mut de = postcard::Deserializer::from_bytes(frame);
        let head = Head::deserialize(&mut de).map_err(SerDeError::from)?;
        let payload = de.finalize().map_err(SerDeError::from)?;

        Ok(match head {
            Head::Notify(method) => InboundFrameType::Notify {
                method: range_of(frame, method.as_bytes()),
                params: range_of(frame, payload),
            },
            Head::Request(request_id, method) => InboundFrameType::Request {
                raw_request_id: range_of(frame, request_id),
                method: range_of(frame, method.as_bytes()),
                params: range_of(frame, payload),
            },
            Head::Response(request_id) => InboundFrameType::Response {
                request_id: self.restore_request_id(request_id)?,
                errc: None,
                payload: range_of(frame, payload),
            },
            Head::ResponseErr(request_id, errc) => InboundFrameType::Response {
                request_id: self.restore_request_id(request_id)?,
                errc: Some(ResponseError::from(errc)),
                payload: range_of(frame, payload),
            },
        })
    }

    fn restore_request_id(
        &self,
        raw_id: &[u8],
    ) -> Result<RequestId, crate::codec::error::DecodeError> {
        RequestId::from_bytes(raw_id).ok_or(DecodeError::InvalidFormat)
    }
}

fn range_of(base: &[u8], cursor: &[u8]) -> std::ops::Range<u32> {
    let p_base = base.as_ptr();
    let p_cursor = cursor.as_ptr();

    assert!(p_base <= p_cursor);

    // SAFETY: We've asserted that p_base <= p_cursor
    let offset = unsafe { p_cursor.offset_from(p_base) };
    let offset = offset as u32;

    let len = cursor.len() as u32;
    assert!(offset + len <= base.len() as u32);

    offset..offset + len
}

fn serialize(buf: &mut BytesMut, item: &impl serde::Serialize) -> Result<(), SerDeError> {
    *buf = serialize_with_flavor(item, ExtendFlavor::new(take(buf))).map_err(SerDeError::from)?;
    Ok(())
}

impl RequestId {
    fn as_bytes(&self) -> &[u8] {
        // ASSERT THIS PROCESS IS LITTLE ENDIAN
        #[cfg(target_endian = "big")]
        compile_error!(
            "This process is big endian; rawrpc is not supported on big endian platform"
        );

        unsafe { std::slice::from_raw_parts((&self.0) as *const _ as *const u8, 8) }
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let id = bytes.get(..8)?;
        let id = u64::from_le_bytes(id.try_into().ok()?);
        Some(Self(NonZeroU64::new(id)?))
    }
}
