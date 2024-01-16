use serde::{Deserialize, Serialize};

pub mod route {
    use thiserror::Error;

    use crate::{Inbound, UserData};

    /// A function which actually deals with inbound message.
    pub type ExecFunc<U> = dyn for<'a> Fn(&mut Option<Inbound<'a, U>>) -> Result<(), ExecError>
        + Send
        + Sync
        + 'static;

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
        ///
        /// # Panics
        ///
        /// This function panics if the inbound message is [`None`].
        pub fn route(
            &self,
            inbound_revoked_on_error: &mut Option<Inbound<'_, U>>,
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
        error::ErrorResponse, Inbound, NotifySender, ParseMessage, RequestSender, UserData,
    };

    use super::{NotifyMethod, RequestMethod};

    pub struct CachedRequest<U: UserData, N: NotifyMethod + RequestMethod> {
        inner: CachedNotify<U, N>,
    }

    pub struct CachedNotify<U: UserData, M: NotifyMethod> {
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
        ib: Inbound<'static, U>,
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
        ) -> Result<Self, erased_serde::Error> {
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
        ) -> Result<Self, erased_serde::Error> {
            Ok(Self {
                // SAFETY:
                // * The borrowed lifetime `'de` is bound to the payload of the inbound message, not
                //   the object itself. Since `CachedNotify` holds the inbound message as long as
                //   the object lives, the lifetime of the payload is also valid.
                // * During the entire lifetime of the parsed object, to ensure that the borrowed
                //   reference is valid, the buffer of the inbound message won't be dropped during
                //   `v`'s lifetime.
                v: unsafe {
                    let msg_ptr = &msg as *const Inbound<'static, U>;
                    transmute((*msg_ptr).parse::<M::ParamRecv<'_>>()?)
                },
                ib: msg,
            })
        }

        pub fn args(&self) -> &M::ParamRecv<'_> {
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
        PhantomData<M>,
    );

    // TODO: Parse API, Deref Inner API
    pub struct CachedErrorResponse<M: RequestMethod>(
        ErrorResponse,
        Result<M::ErrRecv<'static>, erased_serde::Error>,
    );

    // TODO: Parse API, Deref Inner API
    pub struct CachedOkayResponse<M: RequestMethod>(
        crate::Response,
        Result<M::OkRecv<'static>, erased_serde::Error>,
    );

    impl<'a, U, M> std::future::Future for CachedWaitResponse<'a, U, M>
    where
        U: UserData,
        M: RequestMethod,
    {
        type Output = Result<CachedOkayResponse<M>, Option<CachedErrorResponse<M>>>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            // SAFETY: We are not moving the inner value.
            let inner = unsafe { self.map_unchecked_mut(|x| &mut x.0) };

            futures::ready!(inner.poll(cx))
                .map(|x| {
                    let parsed = x.parse::<M::OkRecv<'_>>();
                    todo!()
                })
                .map_err(|x| x.map(|x| todo!()))
                .into()
        }
    }

    // ========================================================== Extensions ===|

    impl<U> NotifySender<U>
    where
        U: UserData,
    {
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
    }

    impl<U> RequestSender<U>
    where
        U: UserData,
    {
        pub async fn call<M>(
            &self,
            buf: &mut BytesMut,
            (_, p): (M, M::ParamSend<'_>),
        ) -> Result<CachedWaitResponse<'_, U, M>, crate::error::SendMsgError>
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
        ) -> Result<CachedWaitResponse<'_, U, M>, crate::error::TrySendMsgError>
        where
            M: NotifyMethod + RequestMethod,
        {
            self.try_request(buf, M::METHOD_NAME, &p)
                .map(|x| CachedWaitResponse(x, PhantomData))
        }
    }
}
