//! # Components
//!
//! - [`transport`] Defines data transport layer.
//! - [`rpc`] Defines RPC layer
//!
pub(crate) mod raw;

pub mod ext;
pub mod rpc;
pub mod transport;

pub use raw::{RepCode, MAX_ROUTE_LEN};
pub use rpc::driver::{InitInfo, Notify, Reply, Request};
pub use rpc::{Handle, Inbound, ReplyWait, WeakHandle};
pub use transport::{AsyncFrameRead, AsyncFrameWrite};

pub mod consts {
    pub const SMALL_PAYLOAD_SLICE_COUNT: usize = 16;

    pub const BUFSIZE_SMALL: usize = 128;
    pub const BUFSIZE_LARGE: usize = 4096;
}

pub mod alias {
    use lockfree_object_pool::{LinearObjectPool, LinearOwnedReusable, LinearReusable};

    pub type PoolPtr<T> = LinearOwnedReusable<T>;
    pub type PoolRef<'a, T> = LinearReusable<'a, T>;
    pub type Pool<T> = LinearObjectPool<T>;

    pub(crate) type AsyncMutex<T> = async_mutex::Mutex<T>;

    pub(crate) fn default<T: Default>() -> T {
        Default::default()
    }
}
