pub mod codec;
pub mod io;
pub mod rpc;
pub mod defs {

    // ========================================================== Basic Types ===|

    use std::{num::NonZeroU32, ops::Range};

    pub(crate) type SizeType = u32;

    /// 32-bit range type. Defines set of helper methods for working with ranges.
    #[derive(Clone, Copy)]
    pub(crate) struct RangeType([SizeType; 2]);

    // ==== RangeType ====

    impl From<Range<usize>> for RangeType {
        fn from(value: Range<usize>) -> Self {
            Self([value.start as SizeType, value.end as SizeType])
        }
    }

    impl From<RangeType> for Range<usize> {
        fn from(value: RangeType) -> Self {
            Self {
                start: value.0[0] as usize,
                end: value.0[1] as usize,
            }
        }
    }

    impl RangeType {
        pub fn new(start: usize, end: usize) -> Self {
            Self([start as SizeType, end as SizeType])
        }

        /// Handy method to get usize range with less typing.
        pub(crate) fn r(&self) -> Range<usize> {
            (*self).into()
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
        pub struct RequestId(NonZeroU32)
    }
}

pub mod ext_io {}
pub mod ext_codec {
    //! Implementations of RPC codecs ([`Codec`]) for various protocols

    #[cfg(feature = "jsonrpc")]
    pub mod jsonrpc {}
    #[cfg(feature = "mspack-rpc")]
    pub mod msgpackrpc {}
    #[cfg(feature = "mspack-rpc-postcard")]
    pub mod msgpackrpc_postcard {}
}
mod inner {
    /// Internal utility to notify that this routine is unlikely to be called.
    #[cold]
    #[inline(always)]
    pub(crate) fn cold_path() {}
}

// ========================================================== Re-exports ===|

// ==== Dependency Crates ====

pub extern crate erased_serde;
pub extern crate serde;

// ==== Exposed APIs ====

pub(crate) use inner::*;

pub use rpc::{
    create_builder, Client, NotifyClient, ReceiveResponse, Response, UserData, WeakClient,
    WeakNotifyClient,
};

pub use codec::{Codec, ParseMessage, ResponseErrorCode};
pub use io::{AsyncFrameRead, AsyncFrameWrite};
