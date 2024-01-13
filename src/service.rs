pub mod route {
    use crate::{Inbound, UserData};

    pub struct Router<U: UserData, R> {
        route: R,
        funcs: Vec<Box<dyn for<'a> Fn(Inbound<'a, U>) + Send + Sync + 'static>>,
    }

    pub trait RouteFunction {
        type BuildError: std::error::Error;
        type Finish: RouteFunction;

        fn add_route(&mut self, path: &str, target: usize) -> Result<(), Self::BuildError>;
        fn finish(self) -> Result<Self::Finish, Self::BuildError>;

        fn route(&self, query: &str) -> Option<usize>;
    }

    impl<U, R> std::fmt::Debug for Router<U, R>
    where
        U: UserData,
        R: std::fmt::Debug,
    {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Router")
                .field("route", &self.route)
                .finish()
        }
    }

    impl<U, R> Router<U, R>
    where
        U: UserData,
        R: RouteFunction,
    {
        pub fn new(router: R) -> Self {
            Self {
                route: router,
                funcs: Default::default(),
            }
        }

        pub fn add_route<F>(&mut self, name: &str, handler: F) -> Result<(), R::BuildError>
        where
            F: Fn() + Send + Sync + 'static,
        {
            todo!()
        }

        pub fn route<'a>(&self, inbound: Inbound<'a, U>) -> Result<(), Inbound<'a, U>> {
            todo!()
        }
    }
}

pub trait RequestMethod {
    type ParamSend<'a>: serde::Serialize;
    type ParamRecv<'de>: serde::Deserialize<'de>;
    type ResultSend<'a>: serde::Serialize;
    type ResultRecv<'de>: serde::Deserialize<'de>;
}

pub trait NotifyMethod {
    type ParamSend<'a>: serde::Serialize;
    type ParamRecv<'de>: serde::Deserialize<'de>;
}

pub mod inbound {
    use std::sync::Arc;

    use bytes::Bytes;

    use crate::{rpc::RpcCore, Inbound, UserData};

    use super::{NotifyMethod, RequestMethod};

    pub struct RequestMessage<U: UserData, M: RequestMethod> {
        request: Inbound<'static, U>,
        param: M::ParamRecv<'static>,
    }

    pub struct NotifyMessage<U: UserData, M: NotifyMethod> {
        core: Arc<dyn RpcCore<U>>,
        data: Bytes,
        param: <M as NotifyMethod>::ParamRecv<'static>,
    }
}
