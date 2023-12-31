pub mod builder {
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    use crate::req::RequestStorage;

    pub struct Builder<Codec, Router, UserData, Write, Read> {
        codec: Codec,
        router: Router,
        user_data: UserData,
        write: Write,
        read: Read,
        reqs: Option<RequestStorage>,
    }

    pub(crate) struct ClientImpl<Codec, UserData, Write> {
        codec: Codec,
        user_data: UserData,
        write: Arc<AsyncMutex<Write>>,
    }

    /// Creates new builder with empty settings.
    ///
    /// A tokio runtime must be running.
    pub fn builder() -> Builder<(), (), (), (), ()> {
        Builder { codec: (), router: (), user_data: (), write: (), read: (), reqs: None }
    }

    impl<T0, T1, T2, T3, T4> Builder<T0, T1, T2, T3, T4> {
        // TODO: Builder struct implementations
    }

    impl<Codec, UserData, Write> crate::client::Client for ClientImpl<Codec, UserData, Write>
    where
        UserData: Send + Sync + 'static,
    {
        type UserData = UserData;
    }

    // TODO: implement receiver task
}
pub mod backend {
    use std::future::Future;

    use bytes::Bytes;
    use thiserror::Error;

    pub trait Backend: Send + Sync + 'static {
        /// Spawn new asynchronous task. Must be succeeded.
        fn spawn_task(&self, task: impl Future<Output = ()> + Send);
    }
}
pub mod client {
    use std::sync::Arc;

    use crate::req::RequestStorage;

    pub(crate) trait Client {
        type UserData: Send + Sync + 'static;
    }

    /// A client that only sends notifications
    #[derive(Clone)]
    pub struct NotifyHandle<UserData> {
        client: Arc<dyn Client<UserData = UserData>>,
    }

    /// A client which can send requests and receive replies
    #[derive(Clone)]
    pub struct Handle<UserData> {
        noti: NotifyHandle<UserData>,
        reqs: Arc<RequestStorage>,
    }
}
pub mod codec {
    use std::num::NonZeroU64;

    pub trait Codec {
        /// Returns unique hash of this codec. If notification encoding is not deterministic,
        /// returns `None`. Otherwise, if two codecs return the same hash, their encoded
        /// notifications are guaranteed to be equal, which is allowed to be reused over multiple
        /// clients.
        ///
        /// This values is used when broadcasting notification message over multiple clients.
        fn notify_type_id(&self) -> Option<NonZeroU64> {
            None
        }

        // TODO: Encode notify

        // TODO: Encode request

        // TODO: Encode reply OK / Error

        // TODO: Decode inbound -> determine type

        // TODO: Retrieve payload from inbound
    }
}
pub mod io {}
pub mod msg {
    pub struct RequestBody {}
}
pub mod req {
    pub struct RequestStorage {
        // TODO: List of awaiting requests

        // TODO: Is reader closed ?
    }
}
pub mod recv {
    pub trait Router {
        // TODO: Handle Error

        // TODO: Handle Request

        // TODO: Handle Notify
    }

    impl Router for () {}
}
pub mod defs {
    pub(crate) type DataSize = u32;
    type ReqIdType = u32;
    pub(crate) type DataRange = std::ops::Range<DataSize>;

    macro_rules! __define_integer_id {
		($(#[doc = $s:literal])* $vis:vis struct $ident:ident($ty:ty)) => {
			$(#[doc = $s])*
			#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
			$vis struct $ident($ty);

			impl $ident {
				#[inline]
				pub fn new(id: $ty) -> Self {
					Self(id)
				}

				#[inline]
				pub fn get(&self) -> $ty {
					self.0
				}
			}
		};
	}

    __define_integer_id!(
        /// The in-memory representation of a request ID.
        pub struct RequestId(ReqIdType)
    );
}

pub use backend::Backend;
pub use client::{Handle, NotifyHandle};

// ======== Re-exports ======== //

pub extern crate erased_serde;
pub extern crate serde;
