use std::{collections::HashMap, error::Error, fmt::Debug};

use serde::{Deserialize, Serialize};

use crate::{codec::DecodeError, rpc::MessageMethodName, Message, Notify, RecvMsg, Request};

pub struct ServiceBuilder<T>(Service<T>);

pub struct Service<T> {
    router: T,
    methods: Vec<InboundHandler>,
}

enum InboundHandler {
    Request(Box<dyn Fn(Request) -> Result<(), RouteMessageError>>),
    Notify(Box<dyn Fn(Notify) -> Result<(), RouteMessageError>>),
}

impl<T: Debug> Debug for Service<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("router", &self.router)
            .field("methods", &self.methods.len())
            .finish()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum RegisterError {
    #[error("Given key is already registered")]
    AlreadyRegistered,
    #[error("Invalid routing key: {0}")]
    InvalidRoutingKey(#[from] Box<dyn Error + Send + Sync + 'static>),
}

pub trait Router: Send + Sync + 'static {
    /// Register a routing key with the given index.
    fn register(&mut self, patterns: &[&str], index: usize) -> Result<(), RegisterError>;

    /// Finish the registration process. All errors must've been reported through `register`, thus
    /// this method should never fail.
    fn finish(&mut self) {}

    /// Route the given routing key to an index.
    fn route(&self, routing_key: &str) -> Option<usize>;
}

impl<T: Default + Router> Default for ServiceBuilder<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> ServiceBuilder<T>
where
    T: Router,
{
    pub fn new(router: T) -> Self {
        Self(Service { router, methods: Vec::new() })
    }

    pub fn register_request_handler<Req, Rep, Err>(
        &mut self,
        patterns: &[&str],
        func: impl Fn(Req, TypedRequest<Rep, Err>) -> Result<(), Box<dyn Error + Send + Sync + 'static>>
            + 'static
            + Send
            + Sync,
    ) where
        Req: for<'de> Deserialize<'de>,
        Rep: Serialize,
        Err: Serialize,
    {
        let index = self.0.methods.len();
        self.0.methods.push(InboundHandler::Request(Box::new(move |request| {
            let request = TypedRequest::<Rep, Err>::new(request);
            let param = match request.parse::<Req>() {
                Ok(x) => x,
                Err(e) => {
                    request.into_request().error_parse_failed_deferred::<Req>().ok();
                    return Err(RouteMessageError::ParseError(e));
                }
            };

            func(param, request)?;
            Ok(())
        })));
        self.0.router.register(patterns, index).unwrap();
    }

    pub fn register_notify_handler<Noti>(
        &mut self,
        patterns: &[&str],
        func: impl Fn(Noti) -> Result<(), Box<dyn Error + Send + Sync + 'static>>
            + 'static
            + Send
            + Sync,
    ) where
        Noti: for<'de> Deserialize<'de>,
    {
        let index = self.0.methods.len();
        self.0.methods.push(InboundHandler::Notify(Box::new(move |request| {
            let param = match request.parse::<Noti>() {
                Ok(x) => x,
                Err(e) => {
                    return Err(RouteMessageError::ParseError(e));
                }
            };

            func(param)?;
            Ok(())
        })));
        self.0.router.register(patterns, index).unwrap();
    }

    pub fn build(mut self) -> Service<T> {
        self.0.router.finish();
        self.0
    }
}

impl<T> Service<T>
where
    T: Router,
{
    pub fn route_message(&self, msg: RecvMsg) -> Result<(), RouteMessageError> {
        let method = std::str::from_utf8(msg.method_raw())?;
        let index = self.router.route(method).ok_or(RouteMessageError::MethodNotFound)?;
        match (self.methods.get(index).ok_or(RouteMessageError::MethodNotFound)?, msg) {
            (InboundHandler::Request(func), RecvMsg::Request(req)) => func(req),
            (InboundHandler::Notify(func), RecvMsg::Notify(noti)) => func(noti),
            (_, RecvMsg::Notify(noti)) => Err(RouteMessageError::NotifyToRequestHandler(noti)),
            (_, RecvMsg::Request(req)) => {
                req.abort_deferred().ok();
                Err(RouteMessageError::RequestToNotifyHandler)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RouteMessageError {
    #[error("Non-utf method name: {0}")]
    NonUtfMethodName(#[from] std::str::Utf8Error),

    #[error("Method couldn't be routed")]
    MethodNotFound,

    #[error("Notify message to request handler")]
    NotifyToRequestHandler(Notify),

    #[error("Request message to notify handler")]
    RequestToNotifyHandler,

    #[error("Failed to parse incoming message: {0}")]
    ParseError(DecodeError),

    #[error("Internal handler returned error")]
    HandlerError(#[from] Box<dyn Error + Send + Sync + 'static>),
}

#[doc(hidden)]
pub mod macro_utils {
    pub type RegisterResult = Result<(), super::RegisterError>;
}

/* ---------------------------------------- Typed Request --------------------------------------- */
#[derive(Debug)]
pub struct TypedRequest<T, E>(Request, std::marker::PhantomData<(T, E)>);

impl<T, E> TypedRequest<T, E>
where
    T: serde::Serialize,
    E: serde::Serialize,
{
    pub fn new(req: Request) -> Self {
        Self(req, Default::default())
    }

    pub fn into_request(self) -> Request {
        self.0
    }

    pub async fn ok_async(self, value: &T) -> Result<(), super::SendError> {
        self.0.response(Ok(value)).await
    }

    pub async fn err_async(self, value: &E) -> Result<(), super::SendError> {
        self.0.response(Err(value)).await
    }

    pub fn ok(self, value: &T) -> Result<(), super::SendError> {
        self.0.response_deferred(Ok(value))
    }

    pub fn err(self, value: &E) -> Result<(), super::SendError> {
        self.0.response_deferred(Err(value))
    }
}

impl<T, E> std::ops::Deref for TypedRequest<T, E> {
    type Target = Request;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/* ------------------------------------ Typed Response Future ----------------------------------- */
#[derive(Debug)]
pub struct TypedResponse<T, E>(crate::OwnedResponseFuture, std::marker::PhantomData<(T, E)>);

/* --------------------------------- Basic Router Implementation -------------------------------- */
#[derive(Debug, Default, Clone)]
pub struct ExactMatchRouter {
    routes: HashMap<String, usize>,
}

impl Router for ExactMatchRouter {
    fn register(&mut self, pattern: &[&str], index: usize) -> Result<(), RegisterError> {
        for pat in pattern.into_iter().copied() {
            if self.routes.contains_key(pat) {
                return Err(RegisterError::AlreadyRegistered);
            }

            self.routes.insert(pat.to_owned(), index);
        }

        Ok(())
    }

    fn route(&self, routing_key: &str) -> Option<usize> {
        self.routes.get(routing_key).copied()
    }
}
