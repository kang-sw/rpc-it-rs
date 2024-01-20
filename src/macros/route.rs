use thiserror::Error;

use crate::{codec::SerDeError, rpc::Config, Inbound};

/// A function which actually deals with inbound message.
pub type ExecFunc<R> =
    dyn Fn(&mut Option<Inbound<R>>) -> Result<(), ExecError> + Send + Sync + 'static;

/// Error from execution function
#[derive(Error, Debug)]
pub enum ExecError {
    #[error("route failed")]
    RouteFailed,

    #[error("parsing inbound failed")]
    ParseError(#[from] SerDeError),

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
    pub fn route(&self, inbound: Inbound<C>) -> Result<(), (Option<Inbound<C>>, ExecError)> {
        let Some(index) = self.route_func.route(inbound.method()) else {
            return Err((Some(inbound), ExecError::RouteFailed));
        };

        let mut opt_inbound = Some(inbound);
        if let Err(e) = self.funcs[index](&mut opt_inbound) {
            Err((opt_inbound, e))
        } else {
            Ok(())
        }
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
