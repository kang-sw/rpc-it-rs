//! # RPC-IT
//!
//! Low-level RPC abstraction
//!
//! # Concepts
//!
//! There are three concepts for RPC handling:
//!
//! - Send Notify
//! - Send Request => Recv Response
//! - Recv Request
//!
//! This library is modeled after the `msgpack-rpc`, but in general, most RPC protocols follow
//! similar patterns, we may adopt this to other protocols in the future. (JSON-RPC, etc...)
//!
//!
//!
//!
//! # Usage
//!
//!
//!

// Re-export crates
pub extern crate bytes;
pub extern crate erased_serde;
pub extern crate futures_util;
pub extern crate serde;

pub mod codec;
pub mod codecs;
pub mod rpc;
pub mod transport;
pub mod transports;

#[cfg(feature = "service")]
pub mod service;

#[cfg(feature = "macros")]
pub mod __util {
    use serde::ser::SerializeMap;

    pub fn iter_as_map<'a, I>(iter: I) -> impl serde::Serialize + 'a
    where
        I: IntoIterator<Item = (&'a dyn erased_serde::Serialize, &'a dyn erased_serde::Serialize)>
            + 'a,
        I::IntoIter: ExactSizeIterator + Clone,
    {
        struct IterAsMap<'a, I>(I)
        where
            I: Iterator<Item = (&'a dyn erased_serde::Serialize, &'a dyn erased_serde::Serialize)>
                + ExactSizeIterator
                + Clone;

        impl<'a, I> serde::Serialize for IterAsMap<'a, I>
        where
            I: Iterator<Item = (&'a dyn erased_serde::Serialize, &'a dyn erased_serde::Serialize)>
                + ExactSizeIterator
                + Clone,
        {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let mut map = serializer.serialize_map(Some(self.0.len()))?;
                for (k, v) in self.0.clone() {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }

        IterAsMap(iter.into_iter())
    }
}

pub use rpc::{
    msg::{Notify, RecvMsg, Request, Response},
    Builder, Feature, InboundError, InboundEventSubscriber, Message, RecvError, RequestContext,
    ResponseFuture, SendError, Sender, Transceiver, TryRecvError,
};

/// Create a map from a list of key-value pairs.
///
/// This is useful to quickly create a fixed-sized map for serialization.
#[cfg(feature = "macros")]
#[macro_export]
macro_rules! kv_pairs {
    ($($key:tt = $value:expr),*) => {
        $crate::__util::iter_as_map([$(
            (&($key) as &dyn $crate::erased_serde::Serialize,
             &($value) as &dyn $crate::erased_serde::Serialize)
        ),*])
    };
}
