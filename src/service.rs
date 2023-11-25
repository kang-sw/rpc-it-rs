use std::{collections::HashMap, error::Error, fmt::Debug};

use crate::{codec::DecodeError, rpc::MessageMethodName, Notify, RecvMsg, Request};

pub struct ServiceBuilder<T = ExactMatchRouter>(Service<T>);

pub struct Service<T = ExactMatchRouter> {
    router: T,
    methods: Vec<InboundHandler>,
}

#[cfg(test)]
static_assertions::assert_impl_all!(Service<ExactMatchRouter>: Send, Sync);

enum InboundHandler {
    Request(Box<dyn Fn(Request) -> Result<(), RouteMessageError> + Send + Sync + 'static>),
    Notify(Box<dyn Fn(Notify) -> Result<(), RouteMessageError> + Send + Sync + 'static>),
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

    pub fn register_request_handler(
        &mut self,
        patterns: &[&str],
        func: impl Fn(Request) -> Result<(), RouteMessageError> + Send + Sync + 'static,
    ) -> Result<(), RegisterError> {
        let index = self.0.methods.len();
        self.0.router.register(patterns, index)?;
        self.0.methods.push(InboundHandler::Request(Box::new(move |request| {
            func(request)?;
            Ok(())
        })));
        Ok(())
    }

    pub fn register_notify_handler(
        &mut self,
        patterns: &[&str],
        func: impl Fn(Notify) -> Result<(), RouteMessageError> + Send + Sync + 'static,
    ) -> Result<(), RegisterError> {
        let index = self.0.methods.len();
        self.0.router.register(patterns, index)?;
        self.0.methods.push(InboundHandler::Notify(Box::new(move |msg| {
            func(msg)?;
            Ok(())
        })));
        Ok(())
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
    NonUtf8MethodName(#[from] std::str::Utf8Error),

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
