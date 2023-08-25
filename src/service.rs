use std::{collections::HashMap, error::Error, fmt::Debug, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{codec::Codec, rpc::msg::TypedRequest, Message, Request};

pub struct ServiceBuilder<T>(Service<T>);

pub struct Service<T> {
    router: T,
    methods: Vec<Box<dyn Fn(Request)>>,
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
pub enum RouterRegisterError {
    #[error("Given key is already registered")]
    AlreadyRegistered,
    #[error("Invalid routing key: {0}")]
    InvalidRoutingKey(#[from] Box<dyn Error + Send + Sync + 'static>),
}

pub trait Router: Send + Sync + 'static {
    /// Register a routing key with the given index.
    fn register(&mut self, patterns: &[&str], index: usize) -> Result<(), RouterRegisterError>;

    /// Finish the registration process. All errors must be reported through `register`, thus this
    /// method should never fail.
    fn finish(&mut self) {}

    /// Route the given routing key to an index.
    fn route(&self, routing_key: &str) -> Option<usize>;
}

impl<T> ServiceBuilder<T>
where
    T: Router,
{
    pub fn new(router: T) -> Self {
        Self(Service { router, methods: Vec::new() })
    }

    pub fn register<'a, Req, Rep, Err, Fun>(
        &mut self,
        patterns: &[&str],
        func: impl Fn(Req, TypedRequest<Rep, Err>),
    ) where
        Req: Deserialize<'a> + 'a,
        Rep: Serialize,
        Err: Serialize,
    {
        let index = self.0.methods.len();
        self.0.methods.push(Box::new(move |req| {
            let typed_req = TypedRequest::<Rep, Err>::new(req);
        }));

        self.0.router.register(patterns, index).unwrap();
    }
}

/* --------------------------------- Basic Router Implementation -------------------------------- */
#[derive(Debug, Default, Clone)]
pub struct ExactMatchRouter {
    routes: HashMap<String, usize>,
}

impl Router for ExactMatchRouter {
    fn register(&mut self, pattern: &[&str], index: usize) -> Result<(), RouterRegisterError> {
        for pat in pattern.into_iter().copied() {
            if self.routes.contains_key(pat) {
                return Err(RouterRegisterError::AlreadyRegistered);
            }

            self.routes.insert(pat.to_owned(), index);
        }

        Ok(())
    }

    fn route(&self, routing_key: &str) -> Option<usize> {
        self.routes.get(routing_key).copied()
    }
}
