use std::{borrow::Cow, sync::Arc};

use bytes::{Bytes, BytesMut};

use crate::{
    defs::{NonzeroRangeType, RangeType},
    Codec, ParseMessage, ResponseError, UserData,
};

use super::{
    error::{SendMsgError, SendResponseError, TrySendMsgError, TrySendResponseError},
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

/// A type of response error that can be sent by [`Inbound::response`].
pub struct ResponsePayload<T: serde::Serialize>(Result<T, (ResponseError, Option<T>)>);

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

    /// Consumes this struct and returns an owned version of it.
    ///
    /// If what you only have is a reference to this struct, you can use [`Inbound::clone_notify`]
    ///
    /// ```no_run
    /// use rpc_it::Inbound;
    ///
    /// fn elevate_inbound<'a>(ib: &mut Inbound<'a, ()>) {
    ///   let owned = ib.take().into_owned();
    /// }
    /// ```
    pub fn into_owned(mut self) -> Inbound<'static, U> {
        Inbound {
            owner: Cow::Owned(Arc::clone(&self.owner)),
            inner: std::mem::take(&mut self.inner),
        }
    }

    /// Retrieves message out of the reference.
    pub fn take(&mut self) -> Inbound<'_, U> {
        todo!()
    }

    fn payload_bytes(&self) -> &[u8] {
        &self.inner.buffer[self.inner.payload.range()]
    }

    /// Clones the notify part of the inbound message. Since response can be sent only once, this
    /// struct does not define generic method for completely cloning the internal state.
    ///
    /// If you want to "retrieve" the request out of this struct, as long as you have a mutable
    /// reference to self, you can use [`Inbound::take`] method to retrieve the request out of this
    /// struct as this defines [`Default`] implementation.
    pub fn clone_notify(&self) -> Inbound<'a, U> {
        Inbound {
            owner: self.owner.clone(),
            inner: InboundDelivery {
                buffer: self.inner.buffer.clone(),
                method: self.inner.method,
                payload: self.inner.payload,
                req_id: None,
            },
        }
    }

    /// Retrieve request atomicly.
    fn atomic_take_request(&self) -> Option<NonzeroRangeType> {
        todo!()
    }

    /// # Panics
    ///
    /// If request is not empty. This is logic error!
    fn atomic_set_request(&self, req_id: NonzeroRangeType) {}

    pub fn is_request(&self) -> bool {
        self.inner.req_id.is_some()
    }

    /// A shorthand for unwrapping result [`Inbound::parse_method`].
    ///
    /// # Panics
    ///
    /// Panics if method name is not a valid UTF-8 string.
    pub fn method(&self) -> &str {
        let bytes = &self.inner.buffer[self.inner.method.range()];

        // If it's not UTF-8 string, it's crate logic error.
        debug_assert!(std::str::from_utf8(bytes).is_ok());

        // SAFETY:
        // * The receiver task firstly verifies if the method name is valid UTF-8 string.
        // * Thus, in this context, the method name bytes is guaranteed to be valid UTF-8 string.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }

    /// Response handle result.
    ///
    /// # Usage
    ///
    /// ```no_run
    /// use rpc_it::{Inbound, ResponseError, BytesMut};
    ///
    /// async fn response_examples(b: &mut BytesMut, ib: &mut Inbound<'_, ()>) {
    ///   // This returns plain ok response, with parameter "hello, world!"
    ///   ib.response(b, Ok("hello, world!"));
    ///
    ///   // This returns predefined error code `ResponseError::MethodNotFound`
    ///   ib.response(b, ResponseError::MethodNotFound);
    ///
    ///   // This will serialize the error object as-is, without specifying `ResponseError`.
    ///   //
    ///   // However, the actual implementation of error serialization is up to the codec.
    ///   ib.response(b, Err("Ta-Da!"));
    ///
    ///   // This is identical with the above example.
    ///   ib.response(b, (ResponseError::Unknown, "Ta-Da!"));
    ///
    ///   // This will serialize the error object in codec-custom way.
    ///   ib.response(b, (ResponseError::MethodNotFound, ib.method()));
    ///
    ///   // NOTE: In practice, this method panics since we can send response ONLY ONCE!
    /// }
    /// ```
    pub async fn response<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        result: impl Into<ResponsePayload<T>>,
    ) -> Result<(), SendResponseError> {
        todo!()
    }

    /// Try to send response.
    ///
    /// # Usage
    ///
    /// See documentation of [`Inbound::response`].
    pub fn try_response<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        result: impl Into<ResponsePayload<T>>,
    ) -> Result<(), TrySendResponseError> {
        todo!()
    }

    /// Drops the request without sending any response. This prevents drop-guard automatically
    /// respond [`ResponseError::Unhandled`].
    pub fn drop_request(&self) {
        todo!()
    }
}

impl<'a, U> Drop for Inbound<'a, U>
where
    U: UserData,
{
    fn drop(&mut self) {
        if let Some(req_id) = todo!() {
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

impl From<ResponseError> for ResponsePayload<()> {
    fn from(code: ResponseError) -> Self {
        Self(Err((code, None)))
    }
}

impl<T> From<Result<T, T>> for ResponsePayload<T>
where
    T: serde::Serialize,
{
    fn from(result: Result<T, T>) -> Self {
        match result {
            Ok(obj) => Self(Ok(obj)),
            Err(obj) => Self(Err((ResponseError::Unknown, Some(obj)))),
        }
    }
}

impl<T> From<(ResponseError, T)> for ResponsePayload<T>
where
    T: serde::Serialize,
{
    fn from((code, obj): (ResponseError, T)) -> Self {
        Self(Err((code, Some(obj))))
    }
}
