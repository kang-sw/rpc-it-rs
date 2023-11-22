use std::{ops::Range, sync::Arc};

use bytes::BytesMut;
use enum_as_inner::EnumAsInner;

use crate::{
    codec::{DecodeError, EncodeError, PredefinedResponseError, ReqId, ReqIdRef},
    rpc::MessageReqId,
};

use super::{Connection, DeferredWrite, ExtractUserData, Message, UserData};

macro_rules! impl_message {
    ($t:ty) => {
        impl super::Message for $t {
            fn payload(&self) -> &[u8] {
                &self.h.buffer[self.h.payload.clone()]
            }

            fn raw(&self) -> &[u8] {
                &self.h.buffer
            }

            fn raw_bytes(&self) -> bytes::Bytes {
                self.h.buffer.clone()
            }

            fn codec(&self) -> &dyn super::Codec {
                &*self.h.codec
            }
        }
    };
}

macro_rules! impl_method_name {
    ($t:ty) => {
        impl super::MessageMethodName for $t {
            fn method_raw(&self) -> &[u8] {
                &self.h.buffer[self.method.clone()]
            }
        }
    };
}

macro_rules! impl_req_id {
    ($t:ty) => {
        impl super::MessageReqId for $t {
            fn req_id(&self) -> ReqIdRef {
                self.req_id.make_ref(&self.h.buffer)
            }
        }
    };
}

/* ------------------------------------- Request Logics ------------------------------------- */
#[derive(Debug)]
pub struct RequestInner {
    pub(super) h: super::InboundBody,
    pub(super) method: Range<usize>,
    pub(super) req_id: ReqId,
}

impl_message!(RequestInner);
impl_method_name!(RequestInner);
impl_req_id!(RequestInner);

#[derive(Debug)]
pub struct Request {
    pub(super) body: Option<(RequestInner, Arc<dyn Connection>)>,
}

impl std::ops::Deref for Request {
    type Target = RequestInner;

    fn deref(&self) -> &Self::Target {
        &self.body.as_ref().unwrap().0
    }
}

impl ExtractUserData for Request {
    fn user_data_raw(&self) -> &dyn UserData {
        self.body.as_ref().unwrap().1.user_data()
    }
    fn user_data_owned(&self) -> super::OwnedUserData {
        super::OwnedUserData(self.body.as_ref().unwrap().1.clone())
    }
    fn extract_sender(&self) -> crate::Sender {
        super::Sender(self.body.as_ref().unwrap().1.clone())
    }
}

impl Request {
    pub async fn response<T: serde::Serialize>(
        self,
        value: Result<&T, &T>,
    ) -> Result<(), super::SendError> {
        let mut buf = Default::default();
        self.response_with_reuse(&mut buf, value).await
    }

    /// Response with given value. If the value is `Err`, the response will be sent as error
    #[doc(hidden)]
    pub async fn response_with_reuse<T: serde::Serialize>(
        self,
        buf: &mut BytesMut,
        value: Result<&T, &T>,
    ) -> Result<(), super::SendError> {
        let conn = self.prepare_response(buf, value)?;
        conn.__write_buffer(buf).await?;
        Ok(())
    }

    /// Explicitly abort the request. This is useful when you want to cancel the request
    pub async fn abort(self) -> Result<(), super::SendError> {
        self.error_predefined(PredefinedResponseError::Aborted).await
    }

    /// Notify handler received notify handler ...
    pub async fn error_notify_handler(self) -> Result<(), super::SendError> {
        self.error_predefined(PredefinedResponseError::NotifyHandler).await
    }

    /// Response with 'parse failed' predefined error type.
    pub async fn error_parse_failed<T>(self) -> Result<(), super::SendError> {
        let err = PredefinedResponseError::ParseFailed(std::any::type_name::<T>().into());
        self.error_predefined(err).await
    }

    /// Response 'internal' error with given error code.
    pub async fn error_internal(
        self,
        errc: i32,
        detail: impl Into<Option<String>>,
    ) -> Result<(), super::SendError> {
        let err = if let Some(detail) = detail.into() {
            PredefinedResponseError::InternalDetailed(errc, detail)
        } else {
            PredefinedResponseError::Internal(errc)
        };

        self.error_predefined(err).await
    }

    async fn error_predefined(
        mut self,
        err: super::codec::PredefinedResponseError,
    ) -> Result<(), super::SendError> {
        let (inner, conn) = self.body.take().unwrap();
        conn.__send_err_predef(&mut Default::default(), &inner, &err).await
    }

    /* ---------------------------------- Deferred Version ---------------------------------- */
    #[doc(hidden)]
    pub fn response_deferred_with_reuse<T: serde::Serialize>(
        self,
        buffer: &mut BytesMut,
        value: Result<&T, &T>,
    ) -> Result<(), super::SendError> {
        buffer.clear();
        let conn = self.prepare_response(buffer, value).unwrap();

        conn.tx_drive()
            .send(super::InboundDriverDirective::DeferredWrite(
                DeferredWrite::Raw(buffer.split().freeze()).into(),
            ))
            .map_err(|_| super::SendError::Disconnected)
    }

    pub fn response_deferred<T: serde::Serialize>(
        self,
        value: Result<&T, &T>,
    ) -> Result<(), super::SendError> {
        self.response_deferred_with_reuse(&mut Default::default(), value)
    }

    pub fn abort_deferred(self) -> Result<(), super::SendError> {
        self.error_predefined_deferred(PredefinedResponseError::Aborted)
    }

    pub fn error_notify_handler_deferred(self) -> Result<(), super::SendError> {
        self.error_predefined_deferred(PredefinedResponseError::NotifyHandler)
    }

    pub fn error_parse_failed_deferred<T>(self) -> Result<(), super::SendError> {
        let err = PredefinedResponseError::ParseFailed(std::any::type_name::<T>().into());
        self.error_predefined_deferred(err)
    }

    pub fn error_internal_deferred(
        self,
        errc: i32,
        detail: impl Into<Option<String>>,
    ) -> Result<(), super::SendError> {
        let err = if let Some(detail) = detail.into() {
            PredefinedResponseError::InternalDetailed(errc, detail)
        } else {
            PredefinedResponseError::Internal(errc)
        };

        self.error_predefined_deferred(err)
    }

    fn error_predefined_deferred(
        mut self,
        err: super::codec::PredefinedResponseError,
    ) -> Result<(), super::SendError> {
        let (inner, conn) = self.body.take().unwrap();
        conn.tx_drive()
            .send(super::InboundDriverDirective::DeferredWrite(
                DeferredWrite::ErrorResponse(inner, err).into(),
            ))
            .map_err(|_| super::SendError::Disconnected)
    }

    /* ------------------------------------ Inner Methods ----------------------------------- */
    fn prepare_response<T: serde::Serialize>(
        mut self,
        buf: &mut BytesMut,
        value: Result<&T, &T>,
    ) -> Result<Arc<dyn Connection>, EncodeError> {
        let (inner, conn) = self.body.take().unwrap();
        buf.clear();

        let encode_as_error = value.is_err();
        let value = value.unwrap_or_else(|x| x);
        inner.codec().encode_response(inner.req_id(), encode_as_error, value, buf)?;

        Ok(conn.clone())
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some((inner, conn)) = self.body.take() {
            conn.tx_drive()
                .send(super::InboundDriverDirective::DeferredWrite(
                    super::DeferredWrite::ErrorResponse(inner, PredefinedResponseError::Unhandled),
                ))
                .ok();
        }
    }
}

/* -------------------------------------- Notify Logics ------------------------------------- */
#[derive(Debug)]
pub struct Notify {
    pub(super) h: super::InboundBody,
    pub(super) method: Range<usize>,
    pub(super) sender: Arc<dyn Connection>,
}

impl_message!(Notify);
impl_method_name!(Notify);

impl ExtractUserData for Notify {
    fn user_data_raw(&self) -> &dyn UserData {
        self.sender.user_data()
    }

    fn user_data_owned(&self) -> super::OwnedUserData {
        super::OwnedUserData(self.sender.clone())
    }

    fn extract_sender(&self) -> crate::Sender {
        super::Sender(self.sender.clone())
    }
}

/* ---------------------------------- Received Message Type --------------------------------- */

#[derive(Debug, EnumAsInner)]
pub enum RecvMsg {
    Request(Request),
    Notify(Notify),
}

impl ExtractUserData for RecvMsg {
    fn user_data_raw(&self) -> &dyn UserData {
        match self {
            Self::Request(x) => x.user_data_raw(),
            Self::Notify(x) => x.user_data_raw(),
        }
    }

    fn user_data_owned(&self) -> super::OwnedUserData {
        match self {
            Self::Request(x) => x.user_data_owned(),
            Self::Notify(x) => x.user_data_owned(),
        }
    }

    fn extract_sender(&self) -> crate::Sender {
        match self {
            Self::Request(x) => x.extract_sender(),
            Self::Notify(x) => x.extract_sender(),
        }
    }
}

impl super::Message for RecvMsg {
    fn payload(&self) -> &[u8] {
        match self {
            Self::Request(x) => x.payload(),
            Self::Notify(x) => x.payload(),
        }
    }

    fn raw(&self) -> &[u8] {
        match self {
            Self::Request(x) => x.raw(),
            Self::Notify(x) => x.raw(),
        }
    }

    fn raw_bytes(&self) -> bytes::Bytes {
        match self {
            Self::Request(x) => x.raw_bytes(),
            Self::Notify(x) => x.raw_bytes(),
        }
    }

    fn codec(&self) -> &dyn crate::codec::Codec {
        match self {
            Self::Request(x) => x.codec(),
            Self::Notify(x) => x.codec(),
        }
    }
}

impl super::MessageMethodName for RecvMsg {
    fn method_raw(&self) -> &[u8] {
        match self {
            Self::Request(x) => x.method_raw(),
            Self::Notify(x) => x.method_raw(),
        }
    }
}

/* ------------------------------------- Response Logics ------------------------------------ */
#[derive(Debug)]
pub struct Response {
    pub(super) h: super::InboundBody,
    pub(super) req_id: ReqId,

    /// Should we interpret the payload as error object?
    pub(super) is_error: bool,
}

impl_message!(Response);
impl_req_id!(Response);

impl Response {
    pub fn is_error(&self) -> bool {
        self.is_error
    }

    pub fn result<'a, T: serde::Deserialize<'a>, E: serde::Deserialize<'a>>(
        &'a self,
    ) -> Result<T, ResponseError<E>> {
        if !self.is_error {
            Ok(self.parse()?)
        } else {
            let codec = self.codec();
            if let Some(predef) = codec.try_decode_predef_error(self.payload()) {
                Err(ResponseError::Predefined(predef))
            } else {
                Err(ResponseError::Typed(self.parse()?))
            }
        }
    }
}

#[derive(EnumAsInner, Debug, thiserror::Error)]
pub enum ResponseError<T> {
    #[error("Requested type returned")]
    Typed(T),

    #[error("Predefined error returned: {0}")]
    Predefined(PredefinedResponseError),

    #[error("Decode error: {0}")]
    DecodeError(#[from] DecodeError),
}
