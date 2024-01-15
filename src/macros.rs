use serde::{Deserialize, Serialize};

pub mod route {

    use thiserror::Error;

    use crate::{Inbound, UserData};

    /// A function which actually deals with inbound message.
    pub type ExecFunc<U> =
        dyn for<'a> Fn(Inbound<'a, U>) -> Result<(), ExecError> + Send + Sync + 'static;

    /// Error from execution function
    #[derive(Error, Debug)]
    pub enum ExecError {
        #[error("route failed")]
        RouteFailed,

        #[error("parsing inbound failed")]
        ParseError(#[from] erased_serde::Error),

        #[error("invalid method type")]
        InvalidMethodType,
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

    pub struct Router<U: UserData, R> {
        route_func: R,
        funcs: Vec<Box<ExecFunc<U>>>,
    }

    pub struct RouterBuilder<U: UserData, R> {
        inner: Router<U, R>,
    }

    // ==== Builder ====

    impl<U, R> Default for RouterBuilder<U, R>
    where
        U: UserData,
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

    impl<U, R> RouterBuilder<U, R>
    where
        U: UserData,
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

        pub fn add_route<F>(&mut self, path: &str, func: F) -> Result<usize, R::Error>
        where
            F: Into<Box<ExecFunc<U>>>,
        {
            let value = self.inner.funcs.len();
            self.inner.route_func.add_route(path, value)?;
            self.inner.funcs.push(func.into());

            Ok(value)
        }

        pub fn add_alias_route(&mut self, alias: &str, index: usize) -> Result<(), R::Error> {
            self.inner.route_func.add_route(alias, index)?;
            Ok(())
        }

        pub fn finish(self) -> Router<U, R::Func> {
            Router {
                route_func: self.inner.route_func.finish(),
                funcs: self.inner.funcs,
            }
        }
    }

    // ==== Router ====

    impl<U, R> Router<U, R>
    where
        U: UserData,
        R: RouterFunc,
    {
        /// Route received inbound to predefined handler function.
        pub fn route(&self, inbound: Inbound<'_, U>) -> Result<(), ExecError> {
            let Some(index) = self.route_func.route(inbound.method()) else {
                return Err(ExecError::RouteFailed);
            };

            self.funcs[index](inbound)?;

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
fn xx() {
    #[derive(Serialize, Deserialize)]
    struct Foo<'a> {
        x: &'a str,
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
    use std::mem::transmute;

    use bytes::BytesMut;

    use crate::{Inbound, NotifySender, RequestSender, UserData};

    use super::{NotifyMethod, RequestMethod};

    pub struct CachedRequest<U: UserData, N: NotifyMethod + RequestMethod> {
        inner: CachedNotify<U, N>,
    }

    pub struct CachedNotify<U: UserData, M: NotifyMethod> {
        ib: Inbound<'static, U>,
        v: M::ParamRecv<'static>,
    }

    struct F;

    impl RequestMethod for F {
        type OkSend<'a> = ();
        type OkRecv<'de> = ();
        type ErrSend<'a> = ();
        type ErrRecv<'de> = &'de str;
    }

    // ==== RequestMessage ====

    impl<U, M> CachedRequest<U, M>
    where
        U: UserData,
        M: RequestMethod + NotifyMethod,
    {
        /// # Safety
        ///
        /// This function is unsafe because it transmutes received parsed value to static lifetime.
        /// To ensure safety, any borrowed reference in the parsed value must be originated from the
        /// buffer of the inbound message.
        #[doc(hidden)]
        pub unsafe fn __internal_create(
            msg: Inbound<'static, U>,
            parsed: M::ParamRecv<'_>,
        ) -> Self {
            Self {
                inner: CachedNotify::__internal_create(msg, parsed),
            }
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

    impl<U, M> std::ops::Deref for CachedRequest<U, M>
    where
        U: UserData,
        M: RequestMethod + NotifyMethod,
    {
        type Target = CachedNotify<U, M>;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    // ==== NotifyMessage ====

    impl<U, M> CachedNotify<U, M>
    where
        U: UserData,
        M: NotifyMethod,
    {
        #[doc(hidden)]
        pub unsafe fn __internal_create(
            msg: Inbound<'static, U>,
            parsed: M::ParamRecv<'_>,
        ) -> Self {
            Self {
                ib: msg,
                v: transmute(parsed),
            }
        }

        pub fn param(&self) -> &M::ParamRecv<'_> {
            unsafe { transmute(&self.v) }
        }
    }

    impl<U, M> std::ops::Deref for CachedNotify<U, M>
    where
        U: UserData,
        M: NotifyMethod,
    {
        type Target = Inbound<'static, U>;

        fn deref(&self) -> &Self::Target {
            &self.ib
        }
    }

    // ========================================================== Response Wait ===|

    pub struct CachedWaitResponse<'a, U: UserData, M: RequestMethod>(
        crate::ReceiveResponse<'a, U>,
        std::marker::PhantomData<M>,
    );

    pub struct CachedErrorResponse<M: RequestMethod>(
        crate::error::ErrorResponse,
        M::ErrRecv<'static>,
    );

    pub struct CachedOkayResponse<M: RequestMethod>(crate::Response, M::OkRecv<'static>);

    impl<'a, U, M> std::future::Future for CachedWaitResponse<'a, U, M>
    where
        U: UserData,
        M: RequestMethod,
    {
        type Output = Result<M::OkRecv<'static>, Option<CachedErrorResponse<M>>>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            // SAFETY: We are not moving the inner value.
            let inner = unsafe { self.map_unchecked_mut(|x| &mut x.0) };

            match futures::ready!(inner.poll(cx)) {
                Ok(response) => {}
                Err(err) => {}
            }

            todo!()
        }
    }

    // ========================================================== Extensions ===|

    impl<U> NotifySender<U>
    where
        U: UserData,
    {
        pub async fn noti<N>(
            &self,
            _: N,
            buf: &mut BytesMut,
            p: &N::ParamSend<'_>,
        ) -> Result<(), crate::error::SendMsgError>
        where
            N: NotifyMethod,
        {
            self.notify(buf, N::METHOD_NAME, &p).await
        }

        pub fn try_noti<N>(
            &self,
            _: N,
            buf: &mut BytesMut,
            p: N::ParamSend<'_>,
        ) -> Result<(), crate::error::TrySendMsgError>
        where
            N: NotifyMethod,
        {
            self.try_notify(buf, N::METHOD_NAME, &p)
        }
    }

    impl<U> RequestSender<U>
    where
        U: UserData,
    {
        pub async fn call<M>(
            &self,
            _: M,
            buf: &mut BytesMut,
            p: &M::ParamSend<'_>,
        ) -> Result<CachedWaitResponse<'_, U, M>, crate::error::SendMsgError>
        where
            M: NotifyMethod + RequestMethod,
        {
            self.request(buf, M::METHOD_NAME, p)
                .await
                .map(|x| CachedWaitResponse(x, std::marker::PhantomData))
        }

        pub async fn try_call<M>(
            &self,
            _: M,
            buf: &mut BytesMut,
            p: &M::ParamSend<'_>,
        ) -> Result<CachedWaitResponse<'_, U, M>, crate::error::TrySendMsgError>
        where
            M: NotifyMethod + RequestMethod,
        {
            self.try_request(buf, M::METHOD_NAME, p)
                .map(|x| CachedWaitResponse(x, std::marker::PhantomData))
        }
    }
}
