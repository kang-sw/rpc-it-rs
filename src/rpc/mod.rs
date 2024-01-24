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

pub use self::core::builder;
pub use self::req_rep::Response;

// ==== Basic RPC ====

use core::RpcCore;

pub trait Config: 'static {
    type Codec: Codec;
    type UserData: UserData;
}

pub struct DefaultConfig<U, C>(std::marker::PhantomData<(U, C)>);

impl<U, C> Config for DefaultConfig<U, C>
where
    U: UserData,
    C: Codec,
{
    type Codec = C;
    type UserData = U;
}

/// A trait constraint for user data type of a RPC connection.
pub trait UserData: std::fmt::Debug + Send + Sync + 'static {}
impl<T> UserData for T where T: std::fmt::Debug + Send + Sync + 'static {}

// ========================================================== Details ===|

pub mod core;
pub mod error;
mod receiver;
mod req_rep;
mod sender;
