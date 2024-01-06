use std::{borrow::Cow, sync::Arc};

use bytes::{Bytes, BytesMut};

use crate::{
    defs::{NonzeroRangeType, RangeType},
    Codec, ParseMessage, ResponseErrorCode, UserData,
};

use super::{
    error::{SendMsgError, TrySendMsgError},
    RpcCore,
};

/// Handles error during receiving inbound messages inside runner.
pub trait ReceiveErrorHandler<T: UserData> {}

/// Default implementation for any type of user data. It ignores every error.
impl<T> ReceiveErrorHandler<T> for () where T: UserData {}

/// A receiver which deals with inbound notifies / requests.
#[derive(Debug)]
pub struct Receiver<U> {
    context: Arc<dyn RpcCore<U>>,
    channel: mpsc::Receiver<InboundDelivery>,
}

// ==== impl:Receiver ====

impl<U> Receiver<U> {}

// ========================================================== Inbound ===|

/// A inbound message that was received from remote. It is either a notification or a request.
pub struct Inbound<'a, U>
where
    U: UserData,
{
    owner: Cow<'a, Arc<dyn RpcCore<U>>>,
    inner: InboundDelivery,
}

/// An inbound message delivered from the background receiver.
#[derive(Default)]
pub(crate) struct InboundDelivery {
    buffer: Bytes,
    method: RangeType,
    payload: RangeType,
    req_id: Option<NonzeroRangeType>,
}

/// Determines which response type to send.
pub enum ResponseErrorAs<T: serde::Serialize> {
    Code(ResponseErrorCode),
    Object(T),
}

// ==== impl:Inbound ====

impl<'a, U> Inbound<'a, U>
where
    U: UserData,
{
    pub fn user_data(&self) -> &U {
        self.owner.user_data()
    }

    pub fn codec(&self) -> &dyn Codec {
        self.owner.codec()
    }

    pub fn cloned_codec(&self) -> Arc<dyn Codec> {
        (*self.owner).clone().self_as_codec()
    }

    /// Convert this inbound into owned lifetime.
    pub fn into_owned(mut self) -> Inbound<'static, U> {
        Inbound {
            owner: Cow::Owned(Arc::clone(&self.owner)),
            inner: std::mem::take(&mut self.inner),
        }
    }

    fn payload_bytes(&self) -> &[u8] {
        &self.inner.buffer[self.inner.payload.range()]
    }

    pub fn is_request(&self) -> bool {
        self.inner.req_id.is_some()
    }

    /// Retrieve raw method bytes
    pub fn method_bytes(&self) -> &[u8] {
        &self.inner.buffer[self.inner.method.range()]
    }

    /// Try to parse method name as UTF-8 string.
    pub fn method(&self) -> Option<&str> {
        std::str::from_utf8(self.method_bytes()).ok()
    }

    /// Try to retrieve request ID. If it's not a request, returns `None`.
    pub fn request_id(&self) -> Option<&[u8]> {
        self.inner
            .req_id
            .as_ref()
            .map(|r| &self.inner.buffer[r.range()])
    }

    /// Response handle result.
    ///
    /// # Panics
    ///
    /// Panics if it's not a request or response was already sent.
    pub async fn response<T: serde::Serialize>(
        &mut self,
        buf: &mut BytesMut,
        result: Result<T, ResponseErrorAs<T>>,
    ) -> Result<(), SendMsgError> {
        todo!()
    }

    /// Try to send response.
    ///
    /// # Panics
    ///
    /// Panics if it's not a request or response was already sent.
    pub fn try_response<T: serde::Serialize>(
        &mut self,
        buf: &mut BytesMut,
        result: Result<T, ResponseErrorAs<T>>,
    ) -> Result<(), TrySendMsgError> {
        todo!()
    }

    /// Drops the request without sending any response. This prevents drop-guard automatically
    /// respond [`ResponseErrorCode::Unhandled`].
    pub fn drop_request(&mut self) {
        self.inner.req_id = None;
    }
}

#[test]
#[ignore]
fn __can_compile_response_into_error() {
    #![allow(warnings)]
    let mut _ib: Inbound<()> = todo!();
    _ib.response(
        &mut Default::default(),
        Err(ResponseErrorCode::Unhandled.into()),
    );
    _ib.response(&mut Default::default(), Err((&()).into()));
}

impl<'a, U> Drop for Inbound<'a, U>
where
    U: UserData,
{
    fn drop(&mut self) {
        if let Some(req_id) = self.request_id() {
            self.owner.on_request_unhandled(req_id);
        }
    }
}

impl<'a, U> ParseMessage for Inbound<'a, U>
where
    U: UserData,
{
    fn codec_payload_pair(&self) -> (&dyn Codec, &[u8]) {
        (self.owner.codec(), self.payload_bytes())
    }
}

// ==== Errors ====

impl From<ResponseErrorCode> for ResponseErrorAs<()> {
    fn from(code: ResponseErrorCode) -> Self {
        Self::Code(code)
    }
}

impl<T> From<T> for ResponseErrorAs<T>
where
    T: serde::Serialize,
{
    fn from(obj: T) -> Self {
        Self::Object(obj)
    }
}
