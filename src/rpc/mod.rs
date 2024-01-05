use std::borrow::Cow;

use std::sync::Arc;
use std::sync::Weak;

use bytes::BytesMut;
use tokio::sync::mpsc;

use crate::codec::error::EncodeError;
use crate::codec::Codec;
use crate::defs::RequestId;

use self::error::*;
use self::req_rep::*;

pub use self::receiver::*;
pub use self::sender::*;

pub use self::builder::create_builder;
pub use self::req_rep::Response;

// ==== Basic RPC ====

/// Generic trait for underlying RPC connection.
pub(crate) trait RpcContext<U: UserData>: std::fmt::Debug {
    fn self_as_codec(self: Arc<Self>) -> Arc<dyn Codec>;
    fn codec(&self) -> &dyn Codec;
    fn user_data(&self) -> &U;

    /// Only available for [`RequestSender`].
    fn shutdown_rx_channel(&self);

    /// Increments the reference count of the [`RequestSender`] handle.
    fn incr_request_sender_refcnt(&self);

    /// Decrements the reference count of the [`RequestSender`] handle. If all request senders are
    /// dropped, the background writer's response handler side will be shut down.
    fn decr_request_sender_refcnt(&self);
}

/// A trait constraint for user data type of a RPC connection.
pub trait UserData: std::fmt::Debug + Send + Sync + 'static {}
impl<T> UserData for T where T: std::fmt::Debug + Send + Sync + 'static {}

// ========================================================== Details ===|

pub mod builder;
pub mod error;
mod req_rep;
mod sender;

mod receiver {
    use std::sync::Arc;

    use super::RpcContext;

    /// A receiver which deals with inbound notifies / requests.
    pub struct Receiver<U> {
        context: Arc<dyn RpcContext<U>>,
    }

    // ========================================================== Service ===|

    impl<U> Receiver<U> {
        // TODO:
    }
}
