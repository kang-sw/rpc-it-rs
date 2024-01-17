pub mod codec;
pub mod io;
pub mod rpc;
pub mod defs {

    // ========================================================== Basic Types ===|

    use std::{
        num::{NonZeroU32, NonZeroU64},
        ops::Range,
        sync::atomic::AtomicU64,
    };

    pub type SizeType = u32;
    pub(crate) type LongSizeType = u64;
    pub(crate) type AtomicLongSizeType = AtomicU64;
    pub(crate) type NonzeroSizeType = NonZeroU32;

    // ========================================================== RangeType ===|

    /// 32-bit range type. Defines set of helper methods for working with ranges.
    #[derive(Default, Clone, Copy)]
    pub(crate) struct RangeType([SizeType; 2]);

    impl RangeType {
        /// Handy method to get usize range with less typing.
        pub(crate) fn range(&self) -> Range<usize> {
            self.0[0] as usize..self.0[1] as usize
        }
    }

    impl<T> From<Range<T>> for RangeType
    where
        T: Into<SizeType>,
    {
        fn from(range: Range<T>) -> Self {
            Self([range.start.into(), range.end.into()])
        }
    }

    #[derive(Clone, Copy)]
    pub(crate) struct NonZeroRangeType(SizeType, NonzeroSizeType);

    impl NonZeroRangeType {
        pub fn new(start: SizeType, end: NonzeroSizeType) -> Self {
            Self(start, end)
        }

        pub fn begin(&self) -> SizeType {
            self.0
        }

        pub fn end(&self) -> NonzeroSizeType {
            self.1
        }

        /// Handy method to get usize range with less typing.
        pub(crate) fn range(&self) -> Range<usize> {
            self.0 as usize..self.1.get() as usize
        }
    }

    // ========================================================== ID Types ===|

    macro_rules! define_id {
		($(#[doc=$doc:literal])* $vis:vis struct $name:ident($base:ty)) => {
			$(#[doc=$doc])*
			#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
			$vis struct $name($base);

            impl $name {
                #[allow(dead_code)]
                pub(crate) fn new(value: $base) -> Self {
                    Self(value)
                }

                pub fn value(&self) -> $base {
                    self.0
                }
            }
		};
	}

    define_id! {
        /// Unique identifier of a RPC request.
        ///
        /// This is basically incremental per connection, and rotates back to 1 after reaching the
        /// maximum value(2^32-1).
        pub struct RequestId(NonZeroU64)
    }
}

#[cfg(feature = "proc-macro")]
pub mod macros;

#[cfg(feature = "proc-macro")]
pub use rpc_it_macros::service;

pub mod ext_codec;
mod inner {
    /// Internal utility to notify that this routine is unlikely to be called.
    #[cold]
    #[inline(always)]
    pub(crate) fn cold_path() {}
}

// ========================================================== Re-exports ===|

// ==== Exported Dependencies ====

pub extern crate bytes;
pub extern crate serde;

// ==== Exposed APIs ====

pub use bytes::{Bytes, BytesMut};

pub(crate) use inner::*;

pub use rpc::{
    builder, error, Config, DefaultConfig, Inbound, NotifySender, ReceiveResponse, Receiver,
    RequestSender, Response, UserData, WeakNotifySender, WeakRequestSender,
};

pub use macros::route::{
    ExecError as RouteExecError, Router, RouterBuilder, RouterFunc, RouterFuncBuilder,
};

#[cfg(feature = "proc-macro")]
pub mod cached {
    pub use crate::macros::inbound::{
        CachedErrorResponse as ErrorResponse, CachedNotify as Notify,
        CachedOkayResponse as OkayResponse, CachedRequest as Request,
        CachedWaitResponse as WaitResponse,
    };
}

#[cfg(feature = "proc-macro")]
pub mod router {
    use crate::{Router, RouterBuilder};

    pub type StdHashMap<R> = Router<R, std::collections::HashMap<String, usize>>;
    pub type StdHashMapBuilder<R> = RouterBuilder<R, std::collections::HashMap<String, usize>>;
    pub type StdBTreeMap<R> = Router<R, std::collections::BTreeMap<String, usize>>;
    pub type StdBTreeMapBuilder<R> = RouterBuilder<R, std::collections::BTreeMap<String, usize>>;
    pub type HashbrownHashMap<R> = Router<R, hashbrown::HashMap<String, usize>>;
    pub type HashbrownHashMapBuilder<R> = RouterBuilder<R, hashbrown::HashMap<String, usize>>;
}

pub use codec::{Codec, ParseMessage, ResponseError};
pub use io::{AsyncFrameRead, AsyncFrameWrite};
