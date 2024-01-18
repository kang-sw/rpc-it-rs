use std::{marker::PhantomData, mem::transmute};

use bytes::BytesMut;

use crate::{
    codec::{error::EncodeError, DeserializeError},
    error::ErrorResponse,
    rpc::{Config, PreparedPacket},
    Codec, Inbound, NotifySender, ParseMessage, RequestSender,
};

use super::{NotifyMethod, RequestMethod};

pub struct CachedRequest<R: Config, N: NotifyMethod + RequestMethod> {
    inner: CachedNotify<R, N>,
}

pub struct CachedNotify<R: Config, M: NotifyMethod> {
    /// NOTE: Paramter order is important; `ib` must be dropped after `v` disposed, as it
    /// borrows the underlying buffer of inbound `ib`
    v: M::ParamRecv<'static>,

    ///
    ///
    /// **_WARNING_**
    ///
    /// !!! KEEP `ib` AS THE LAST FIELD !!!
    ///
    /// **_WARNING_**
    ///
    ///
    /// This field should never be exposed as mutable reference.
    ib: Inbound<R>,
}

struct F;

impl RequestMethod for F {
    type OkSend<'a> = ();
    type OkRecv<'de> = ();
    type ErrSend<'a> = ();
    type ErrRecv<'de> = &'de str;
}

// ==== RequestMessage ====

impl<R, M> CachedRequest<R, M>
where
    R: Config,
    M: RequestMethod + NotifyMethod,
{
    /// # Safety
    ///
    /// This function is unsafe because it transmutes received parsed value to static lifetime.
    /// To ensure safety, any borrowed reference in the parsed value must be originated from the
    /// buffer of the inbound message.
    #[doc(hidden)]
    pub unsafe fn __internal_create(
        msg: Inbound<R>,
    ) -> Result<Self, (Inbound<R>, DeserializeError)> {
        Ok(Self {
            inner: CachedNotify::__internal_create(msg)?,
        })
    }

    pub async fn response<'a>(
        &self,
        buf: &mut BytesMut,
        result: Result<M::OkSend<'a>, M::ErrSend<'a>>,
    ) -> Result<(), crate::error::SendResponseError> {
        match result {
            Ok(msg) => self.inner.ib.response(buf, Ok(&msg)).await,
            Err(msg) => self.inner.ib.response(buf, Err(msg)).await,
        }
    }

    pub fn try_response<'a>(
        &self,
        buf: &mut BytesMut,
        result: Result<M::OkSend<'a>, M::ErrSend<'a>>,
    ) -> Result<(), crate::error::TrySendResponseError> {
        match result {
            Ok(msg) => self.inner.ib.try_response(buf, Ok(&msg)),
            Err(msg) => self.inner.ib.try_response(buf, Err(msg)),
        }
    }
}

impl<R, M> std::ops::Deref for CachedRequest<R, M>
where
    R: Config,
    M: RequestMethod + NotifyMethod,
{
    type Target = CachedNotify<R, M>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// ==== NotifyMessage ====

impl<R, M> CachedNotify<R, M>
where
    R: Config,
    M: NotifyMethod,
{
    #[doc(hidden)]
    pub unsafe fn __internal_create(
        msg: Inbound<R>,
    ) -> Result<Self, (Inbound<R>, DeserializeError)> {
        Ok(Self {
            // SAFETY:
            // * The borrowed lifetime `'de` is bound to the payload of the inbound message, not
            //   the object itself. Since `CachedNotify` holds the inbound message as long as
            //   the object lives, the lifetime of the payload is also valid.
            // * During the entire lifetime of the parsed object, to ensure that the borrowed
            //   reference is valid, the buffer of the inbound message won't be dropped during
            //   `v`'s lifetime.
            v: unsafe {
                let msg_ptr = &msg as *const Inbound<R>;
                transmute(match (*msg_ptr).parse::<M::ParamRecv<'_>>() {
                    Ok(ok) => ok,
                    Err(err) => return Err((msg, err)),
                })
            },
            ib: msg,
        })
    }

    pub fn args(&self) -> &M::ParamRecv<'_> {
        // SAFETY: We're downgrading the lifetime of the object, which is safe as long as the
        // `self` object is alive; where identical with the lifetime that we're returning.
        unsafe { transmute(&self.v) }
    }
}

impl<R, M> std::ops::Deref for CachedNotify<R, M>
where
    R: Config,
    M: NotifyMethod,
{
    type Target = Inbound<R>;

    fn deref(&self) -> &Self::Target {
        &self.ib
    }
}

// ========================================================== Response Wait ===|

pub struct CachedWaitResponse<'a, R: Config, M: RequestMethod>(
    crate::ReceiveResponse<'a, R>,
    PhantomData<M>,
);

pub struct CachedErrorResponse<C: Codec, M: RequestMethod>(
    ErrorResponse<C>,
    Result<M::ErrRecv<'static>, DeserializeError>,
);

pub struct CachedOkayResponse<C: Codec, M: RequestMethod>(
    crate::Response<C>,
    Result<M::OkRecv<'static>, DeserializeError>,
);

impl<'a, R, M> std::future::Future for CachedWaitResponse<'a, R, M>
where
    R: Config,
    M: RequestMethod,
{
    type Output = Result<CachedOkayResponse<R::Codec, M>, Option<CachedErrorResponse<R::Codec, M>>>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: We are not moving the inner value.
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.0) };

        // SAFETY: See the comment in `CachedNotify::__internal_create`
        futures::ready!(inner.poll(cx))
            .map(|x| unsafe {
                let parsed = (*(&x as *const crate::Response<R::Codec>)).parse::<M::OkRecv<'_>>();
                CachedOkayResponse(x, transmute(parsed))
            })
            .map_err(|x| {
                x.into_response().map(|x| unsafe {
                    let parsed =
                        (*(&x as *const ErrorResponse<R::Codec>)).parse::<M::ErrRecv<'_>>();
                    CachedErrorResponse(x, transmute(parsed))
                })
            })
            .into()
    }
}

impl<C, F> std::ops::Deref for CachedOkayResponse<C, F>
where
    C: Codec,
    F: RequestMethod,
{
    type Target = crate::Response<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C, F> std::ops::Deref for CachedErrorResponse<C, F>
where
    C: Codec,
    F: RequestMethod,
{
    type Target = ErrorResponse<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Codec, F> CachedOkayResponse<C, F>
where
    F: RequestMethod,
{
    pub fn result(&self) -> &Result<F::OkRecv<'_>, DeserializeError> {
        // SAFETY: See the comment in `CachedNotify::args`
        unsafe { transmute(&self.1) }
    }
}

impl<C: Codec, F> CachedErrorResponse<C, F>
where
    F: RequestMethod,
{
    pub fn result(&self) -> &Result<F::ErrRecv<'_>, DeserializeError> {
        // SAFETY: See the comment in `CachedNotify::args`
        unsafe { transmute(&self.1) }
    }
}

// ========================================================== Extensions ===|

impl<R: Config> NotifySender<R> {
    pub async fn noti<M>(
        &self,
        buf: &mut BytesMut,
        (_, p): (M, M::ParamSend<'_>),
    ) -> Result<(), crate::error::SendMsgError>
    where
        M: NotifyMethod,
    {
        self.notify(buf, M::METHOD_NAME, &p).await
    }

    pub fn try_noti<M>(
        &self,
        buf: &mut BytesMut,
        (_, p): (M, M::ParamSend<'_>),
    ) -> Result<(), crate::error::TrySendMsgError>
    where
        M: NotifyMethod,
    {
        self.try_notify(buf, M::METHOD_NAME, &p)
    }

    pub fn prepare<M>(
        &self,
        buf: &mut BytesMut,
        (_, p): (M, M::ParamSend<'_>),
    ) -> Result<PreparedPacket<R::Codec>, EncodeError>
    where
        M: NotifyMethod,
    {
        self.encode_notify(buf, M::METHOD_NAME, &p)
    }
}

impl<R: Config> RequestSender<R> {
    pub async fn call<M>(
        &self,
        buf: &mut BytesMut,
        (_, p): (M, M::ParamSend<'_>),
    ) -> Result<CachedWaitResponse<'_, R, M>, crate::error::SendMsgError>
    where
        M: NotifyMethod + RequestMethod,
    {
        self.request(buf, M::METHOD_NAME, &p)
            .await
            .map(|x| CachedWaitResponse(x, PhantomData))
    }

    pub fn try_call<M>(
        &self,
        buf: &mut BytesMut,
        (_, p): (M, M::ParamSend<'_>),
    ) -> Result<CachedWaitResponse<'_, R, M>, crate::error::TrySendMsgError>
    where
        M: NotifyMethod + RequestMethod,
    {
        self.try_request(buf, M::METHOD_NAME, &p)
            .map(|x| CachedWaitResponse(x, PhantomData))
    }
}
