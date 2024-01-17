use serde::{Deserialize, Serialize};

pub mod route {
    use thiserror::Error;

    use crate::{codec::DeserializeError, rpc::Config, Inbound};

    /// A function which actually deals with inbound message.
    pub type ExecFunc<R> =
        dyn Fn(&mut Option<Inbound<R>>) -> Result<(), ExecError> + Send + Sync + 'static;

    /// Error from execution function
    #[derive(Error, Debug)]
    pub enum ExecError {
        #[error("route failed")]
        RouteFailed,

        #[error("parsing inbound failed")]
        ParseError(#[from] DeserializeError),

        #[error("request received on notify handler!")]
        RequestOnNotifyHandler,
    }

    /// A function which routes method(i.e. query) to internal index representation. See
    /// [`RouterFuncBuilder`] for more details.
    pub trait RouterFunc {
        fn route(&self, query: &str) -> Option<usize>;
    }

    /// A router function builder.
    pub trait RouterFuncBuilder {
        type Error;
        type Func: RouterFunc;

        /// Add route to the builder with given expression. The expression can be any valid UTF-8
        /// string. If the expression is already registered or not compilable, this function should
        /// return an error.
        fn add_route(&mut self, expression: &str, value: usize) -> Result<(), Self::Error>;
        fn finish(self) -> Self::Func;
    }

    pub struct Router<C: Config, R> {
        route_func: R,
        funcs: Vec<Box<ExecFunc<C>>>,
    }

    pub struct RouterBuilder<C: Config, R> {
        inner: Router<C, R>,
    }

    // ==== Builder ====

    impl<C: Config, R> Default for RouterBuilder<C, R>
    where
        R: RouterFuncBuilder + Default,
    {
        fn default() -> Self {
            Self {
                inner: Router {
                    route_func: R::default(),
                    funcs: vec![],
                },
            }
        }
    }

    impl<C: Config, R> RouterBuilder<C, R>
    where
        R: RouterFuncBuilder,
    {
        pub fn new(builder: R) -> Self {
            Self {
                inner: Router {
                    route_func: builder,
                    funcs: vec![],
                },
            }
        }

        pub fn push_handler<F>(&mut self, func: F)
        where
            F: Fn(&mut Option<Inbound<C>>) -> Result<(), ExecError> + Send + Sync + 'static,
        {
            self.inner.funcs.push(Box::new(func));
        }

        /// # Panics
        ///
        /// If no handler is registered, this function panics.
        pub fn try_add_route_to_last(&mut self, alias: &str) -> Result<(), R::Error> {
            self.inner
                .route_func
                .add_route(alias, self.inner.funcs.len() - 1)?;

            Ok(())
        }

        pub fn pop_handler(&mut self) {
            self.inner.funcs.pop();
        }

        pub fn try_add_routed_handler<F>(&mut self, path: &str, func: F) -> Result<usize, R::Error>
        where
            F: Into<Box<ExecFunc<C>>>,
        {
            let value = self.inner.funcs.len();
            self.inner.route_func.add_route(path, value)?;
            self.inner.funcs.push(func.into());

            Ok(value)
        }

        pub fn finish(self) -> Router<C, R::Func> {
            Router {
                route_func: self.inner.route_func.finish(),
                funcs: self.inner.funcs,
            }
        }
    }

    // ==== Router ====

    impl<C: Config, R> Router<C, R>
    where
        R: RouterFunc,
    {
        /// Route received inbound to predefined handler function.
        ///
        /// # Panics
        ///
        /// This function panics if the inbound message is [`None`].
        pub fn route(
            &self,
            inbound_revoked_on_error: &mut Option<Inbound<C>>,
        ) -> Result<(), ExecError> {
            let opt_inbound = inbound_revoked_on_error;

            let Some(inbound) = opt_inbound else {
                panic!(
                    "Inbound message is None.\
                     The inbound message is 'Option' to avoid unnecessary cloning, not for \
                     indicating the message is optional!"
                );
            };

            let Some(index) = self.route_func.route(inbound.method()) else {
                return Err(ExecError::RouteFailed);
            };

            self.funcs[index](opt_inbound)?;

            // On successful invocation, the inbound message should be taken out.
            opt_inbound.take();

            Ok(())
        }
    }

    // ==== Route Functions ====

    macro_rules! define_for {
        ($($args:tt)*) => {
            impl<S> RouterFuncBuilder for $($args)*<S, usize>
            where
                S: std::hash::Hash
                    + PartialEq
                    + Eq
                    + PartialOrd
                    + Ord
                    + for<'a> From<&'a str>
                    + std::borrow::Borrow<str>,
            {
                type Error = ();
                type Func = Self;

                fn add_route(&mut self, path: &str, value: usize) -> Result<(), Self::Error> {
                    if self.insert(path.into(), value).is_none() {
                        Ok(())
                    } else {
                        Err(())
                    }
                }

                fn finish(self) -> Self::Func {
                    self
                }
            }

            impl<S> RouterFunc for $($args)*<S, usize>
            where
                S: std::hash::Hash + PartialEq + Eq + PartialOrd + Ord + std::borrow::Borrow<str>,
            {
                fn route(&self, query: &str) -> Option<usize> {
                    self.get(query).copied()
                }
            }
        };
    }

    define_for!(std::collections::HashMap);
    define_for!(std::collections::BTreeMap);
    define_for!(hashbrown::HashMap);

    // ========================================================== Macro Util ===|

    pub enum RouteFailResponse {
        /// Just ignore this error, and continue to route.
        IgnoreAndContinue,
        /// Generate panic. This is default behavior
        Panic,
        /// Abort route installation process. The router instance will be leaved in corrupted state.
        Abort,
    }

    impl From<()> for RouteFailResponse {
        fn from(_: ()) -> Self {
            Self::Panic
        }
    }

    impl From<bool> for RouteFailResponse {
        fn from(value: bool) -> Self {
            if value {
                Self::IgnoreAndContinue
            } else {
                Self::Abort
            }
        }
    }
}

pub trait RequestMethod {
    type OkSend<'a>: Serialize;
    type OkRecv<'de>: Deserialize<'de>;
    type ErrSend<'a>: Serialize;
    type ErrRecv<'de>: Deserialize<'de>;
}

pub trait NotifyMethod {
    type ParamSend<'a>: Serialize;
    type ParamRecv<'de>: Deserialize<'de>;

    const METHOD_NAME: &'static str;
}

/// Tries to verify if we can generate following code
#[test]
#[ignore]
#[cfg(feature = "jsonrpc")]
fn xx() {
    #![allow(unused_parens)]

    #[derive(Serialize)]
    struct Foo<'a> {
        x: &'a str,
    }
    #[derive(Serialize, Deserialize)]
    struct A {
        i: i32,
        vv: f32,
        k: Vec<i32>,
    }

    impl<'de> serde::Deserialize<'de> for Foo<'de> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let (x) = <(&str) as serde::Deserialize>::deserialize(deserializer)?;

            Ok(Self { x })
        }
    }

    struct MyMethod;

    impl NotifyMethod for MyMethod {
        type ParamSend<'a> = Foo<'a>;
        type ParamRecv<'de> = Foo<'de>;

        const METHOD_NAME: &'static str = "my_method";
    }

    let payload = "{}";
    let _ = serde_json::from_str::<<MyMethod as NotifyMethod>::ParamRecv<'_>>(payload);
}

pub mod inbound {
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
        type Output =
            Result<CachedOkayResponse<R::Codec, M>, Option<CachedErrorResponse<R::Codec, M>>>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            // SAFETY: We are not moving the inner value.
            let inner = unsafe { self.map_unchecked_mut(|x| &mut x.0) };

            // SAFETY: See the comment in `CachedNotify::__internal_create`
            futures::ready!(inner.poll(cx))
                .map(|x| unsafe {
                    let parsed =
                        (*(&x as *const crate::Response<R::Codec>)).parse::<M::OkRecv<'_>>();
                    CachedOkayResponse(x, transmute(parsed))
                })
                .map_err(|x| {
                    x.map(|x| unsafe {
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
}
