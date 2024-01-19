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
    ) -> Result<(), crate::codec::error::EncodeError> {
        todo!()
    }

    fn encode_request<S: serde::Serialize>(
        &self,
        request_id: crate::defs::RequestId,
        method: &str,
        params: &S,
        buf: &mut bytes::BytesMut,
    ) -> Result<crate::defs::RequestId, crate::codec::error::EncodeError> {
        todo!()
    }

    fn encode_response<S: serde::Serialize>(
        &self,
        request_id_raw: &[u8],
        result: crate::codec::EncodeResponsePayload<S>,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        todo!()
    }

    fn payload_deserializer<'de>(
        &'de self,
        payload: &'de [u8],
    ) -> Result<impl crate::codec::AsDeserializer<'de>, crate::codec::DecodePayloadUnsupportedError>
    {
        todo!()
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
        todo!()
    }
}
