use std::borrow::Cow;

use std::sync::Arc;
use std::sync::Weak;

use bytes::BytesMut;

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
///
/// It hides complicated generic types and provides a unified interface for RPC connection.
trait RpcCore<U>: std::fmt::Debug {
    fn self_as_codec(self: Arc<Self>) -> Arc<dyn Codec>;
    fn codec(&self) -> &dyn Codec;
    fn user_data(&self) -> &U;

    /// Borrow `tx_deferred` channel. It only returns valid reference if the writer is defined.
    ///
    /// For [`NotifySender`] reference,  it is always defined.
    fn tx_deferred(&self) -> Option<&mpsc::Sender<DeferredDirective>>;

    /// Conditionally retrieves request context
    fn request_context(&self) -> Option<&RequestContext>;
}

/// A trait constraint for user data type of a RPC connection.
pub trait UserData: std::fmt::Debug + Send + Sync + 'static {}
impl<T> UserData for T where T: std::fmt::Debug + Send + Sync + 'static {}

// ========================================================== Details ===|

pub mod builder;
pub mod error;
mod receiver;
mod req_rep;
mod sender;
