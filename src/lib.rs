//! # Components
//!
//! - [`transport`] Defines data transport layer.
//! - [`rpc`] Defines RPC layer
//!
pub(crate) mod raw;

pub mod ext;
pub mod rpc;
pub mod transport;

pub use raw::RepCode;
pub use rpc::driver::InitInfo;
pub use rpc::{Handle, Inbound, ReplyWait, WeakHandle};
pub use transport::{AsyncFrameRead, AsyncFrameWrite};

pub mod alias {
    use lockfree_object_pool::{LinearObjectPool, LinearOwnedReusable, LinearReusable};
    use std::sync::Arc;

    pub type PoolPtr<T> = LinearOwnedReusable<T>;
    pub type PoolRef<'a, T> = LinearReusable<'a, T>;
    pub type Pool<T> = Arc<LinearObjectPool<T>>;

    pub(crate) type AsyncMutex<T> = async_mutex::Mutex<T>;

    pub(crate) fn default<T: Default>() -> T {
        Default::default()
    }
}
