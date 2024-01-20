use std::{marker::PhantomData, mem::transmute};

use bytes::BytesMut;
use thiserror::Error;

use crate::{
    codec::{error::EncodeError, SerDeError},
    error::{ErrorResponse, ResponseReceiveError},
    rpc::{Config, PreparedPacket},
    Codec, Inbound, NotifySender, ParseMessage, RequestSender, Response,
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
    #[doc(hidden)]
    pub fn __internal_create(msg: Inbound<R>) -> Result<Self, (Inbound<R>, SerDeError)> {
        Ok(Self {
            inner: CachedNotify::__internal_create(msg)?,
        })
    }

    pub async fn respond<'a>(
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
    pub fn __internal_create(msg: Inbound<R>) -> Result<Self, (Inbound<R>, SerDeError)> {
        // SAFETY:
        // * The borrowed lifetime `'de` is bound to the payload of the inbound message, not
        //   the object itself. Since `CachedNotify` holds the inbound message as long as
        //   the object lives, the lifetime of the payload is also valid.
        // * During the entire lifetime of the parsed object, to ensure that the borrowed
        //   reference is valid, the buffer of the inbound message won't be dropped during
        //   `v`'s lifetime.
        // * See the comment in `ref_as_static`
        unsafe {
            let (msg, parsed) = parsed_pair::<_, _, M::ParamRecv<'static>>(msg);

            Ok(Self {
                v: transmute(match parsed {
                    Ok(ok) => ok,
                    Err(err) => return Err((msg, err)),
                }),
                ib: msg,
            })
        }
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

pub struct CachedErrorObj<C: Codec, M: RequestMethod>(ErrorResponse<C>, M::ErrRecv<'static>);

pub struct CachedOkayObj<C: Codec, M: RequestMethod>(crate::Response<C>, M::OkRecv<'static>);

#[derive(Error)]
pub enum CachedWaitError<C: Codec, M: RequestMethod> {
    #[error("A receive channel was closed. No more message to consume!")]
    Closed,

    #[error("No message available")]
    Empty,

    #[error("Already retrieved the result.")]
    Retrieved,

    #[error("Remote peer respond with an error")]
    Response(CachedErrorObj<C, M>),

    #[error("Parse error during deserialization: {0:?}")]
    DeserializeFailed(SerDeError, Result<Response<C>, ErrorResponse<C>>),
}

impl<'a, R, M> std::future::Future for CachedWaitResponse<'a, R, M>
where
    R: Config,
    M: RequestMethod,
{
    type Output = Result<CachedOkayObj<R::Codec, M>, CachedWaitError<R::Codec, M>>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: We are not moving the inner value.
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.0) };

        // SAFETY: See the comment in `CachedNotify::__internal_create`

        match futures::ready!(inner.poll(cx)) {
            Ok(resp) => unsafe {
                let (resp, parse_result) = parsed_pair::<_, _, M::OkRecv<'static>>(resp);
                match parse_result {
                    Ok(parsed) => Ok(CachedOkayObj(resp, transmute(parsed))),
                    Err(err) => Err(CachedWaitError::DeserializeFailed(err, Ok(resp))),
                }
            },
            Err(err) => {
                let err = match err {
                    ResponseReceiveError::Closed => CachedWaitError::Closed,
                    ResponseReceiveError::Empty => CachedWaitError::Empty,
                    ResponseReceiveError::Retrieved => CachedWaitError::Retrieved,
                    ResponseReceiveError::Response(resp) => unsafe {
                        // SAFETY: See the comment in `CachedNotify::__internal_create`
                        let (resp, parse_result) = parsed_pair::<_, _, M::ErrRecv<'static>>(resp);
                        match parse_result {
                            Ok(parsed) => {
                                CachedWaitError::Response(CachedErrorObj(resp, transmute(parsed)))
                            }
                            Err(err) => CachedWaitError::DeserializeFailed(err, Err(resp)),
                        }
                    },
                };

                Err(err)
            }
        }
        .into()
    }
}

impl<C, F> std::ops::Deref for CachedOkayObj<C, F>
where
    C: Codec,
    F: RequestMethod,
{
    type Target = crate::Response<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C, F> std::ops::Deref for CachedErrorObj<C, F>
where
    C: Codec,
    F: RequestMethod,
{
    type Target = ErrorResponse<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Codec, F> CachedOkayObj<C, F>
where
    F: RequestMethod,
{
    pub fn value(&self) -> &F::OkRecv<'_> {
        // SAFETY: See the comment in `CachedNotify::args`
        unsafe { transmute(&self.1) }
    }
}

impl<C: Codec, F> CachedErrorObj<C, F>
where
    F: RequestMethod,
{
    pub fn value(&self) -> &F::ErrRecv<'_> {
        // SAFETY: See the comment in `CachedNotify::args`
        unsafe { transmute(&self.1) }
    }
}

impl<C: Codec, F: RequestMethod> std::fmt::Debug for CachedWaitError<C, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedWaitResponseError").finish()
    }
}

impl<C: Codec, F: RequestMethod> CachedWaitError<C, F> {
    pub fn into_response(self) -> Option<CachedErrorObj<C, F>> {
        if let Self::Response(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn as_response(&self) -> Option<&CachedErrorObj<C, F>> {
        if let Self::Response(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

impl<C: Codec, F: RequestMethod> std::fmt::Debug for CachedErrorObj<C, F>
where
    F::ErrRecv<'static>: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CachedErrorResponse")
            .field(&self.0)
            .field(&self.1)
            .finish()
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

    #[must_use = "Prepared packet won't do anything unless sent"]
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

unsafe fn parsed_pair<
    C: Codec,
    P: ParseMessage<C> + 'static,
    D: serde::de::Deserialize<'static>,
>(
    p: P,
) -> (P, Result<D, SerDeError>) {
    // From the parser's perspective, the lifetime of original buffer is 'static until it
    // gets dropped. It's not clear if the parsing behavior differentiates whether borrowing
    // buffer is 'static or not, just to be safe, we're transmuting the lifetime to 'static
    // here, let parser behave consistently.

    let ptr: *const _ = &p;
    let sref: &'static _ = &*ptr;

    let de = sref.parse::<'static, D>();
    (p, de)
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
