//! # API Design
//!
//! ## Request
//! ``` plain
//! let result: MyResponseType = rpc.request("my_method/name?key1=value1", MyRequestType { ... }).await;
//! ```
//!
//! ## Notify
//! ```plain
//! ```
//!
//! ## Response
//! ``` plain
//! let rx_request = rpc_builder.route::<MyRequestType, MyResponseType>("my_method/<name>");
//!
//! let mut req = rx_request.recv().await?;
//! let client = req.client();
//! let name = req.path("name")?;
//! if let Some(age) = req.query("age") {
//!     // ...
//! }
//!
//! req.reply(MyResponseType { ... }).await?;
//! ```
//!

pub mod server {
    use std::marker::PhantomData;

    pub struct Service {}

    impl Service {
        pub fn route<Req, Rep>(
            &mut self,
            route_rule: &str,
        ) -> Result<RequestReceiver<Req, Rep>, RouteError> {
            todo!()
        }

        pub fn merge(&mut self, other: Self) -> Result<(), MergeError> {
            todo!()
        }
    }

    pub struct RequestReceiver<Req, Rep> {
        _0: PhantomData<(Req, Rep)>,
    }

    #[derive(Debug, thiserror::Error)]
    pub enum RouteError {
        #[error("invalid url: {0}")]
        InvalidUrl(String),

        #[error("duplicate url: {0}")]
        DuplicateUrl(String),
    }

    #[derive(Debug, thiserror::Error)]
    pub enum MergeError {}
}

pub mod client {
    pub struct Rpc {}

    impl Rpc {
        pub async fn request<Req, Rep>(&self, url: &str, param: &Req) -> Result<Rep, RequestError> {
            todo!()
        }

        pub async fn notify<T>(&self, url: &str, param: &T) -> Result<(), NotifyError> {
            todo!()
        }
    }

    pub enum RequestError {}
    pub enum NotifyError {}
}
