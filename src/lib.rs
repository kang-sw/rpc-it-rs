//! # Components
//!
//! - [`transport`] Defines data transport layer.
//! - [`rpc`] Defines RPC layer
//!
pub(crate) mod raw;

mod ext;
pub use ext::*; // As all internal modules are prefixed with `ext_`, this is safe.

pub mod rpc;
pub mod transport;

pub use raw::{ReplyCode, MAX_ROUTE_LEN};
pub use rpc::driver::{InitInfo, Notify, PooledBuffer, Reply, Request, VecPool};
pub use rpc::{Handle, Inbound, ReplyWait, WeakHandle};
pub use traits::{RetrievePayload, RetrieveRoute};
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

pub mod traits {
    use std::ffi::CStr;

    pub trait RetrievePayload {
        fn payload(&self) -> &[u8];
    }

    pub trait RetrieveRoute {
        fn route(&self) -> &[u8];
        fn route_str(&self) -> Option<&str> {
            std::str::from_utf8(self.route()).ok()
        }
        unsafe fn route_cstr(&self) -> Option<&CStr> {
            let route = self.route();
            if route.is_empty() {
                return None;
            }

            let cs = CStr::from_ptr(route.as_ptr() as *const _);
            (cs.to_bytes().len() <= route.len()).then(|| cs)
        }
    }
}
