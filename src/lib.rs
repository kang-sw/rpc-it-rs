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
pub mod rpc;

pub use rpc::{
    msg::{Notify, RecvMsg, Request, Response},
    Builder, Channel, Client, Feature, InboundError, InboundEventSubscriber, Message, RecvError,
    RequestContext, ResponseFuture, SendError, TryRecvError,
};

pub mod transport {
    pub use bytes::Bytes;
    pub use futures_util::{AsyncRead, AsyncWrite, Stream};

    pub type InboundChunk = std::io::Result<Bytes>;
}

pub mod ext_transport {
    #[cfg(feature = "tokio")]
    mod tokio_ {}

    #[cfg(feature = "tokio-tungstenite")]
    mod tokio_tungstenite_ {}

    #[cfg(feature = "wasm-bindgen-ws")]
    mod wasm_websocket_ {}

    #[cfg(feature = "in-memory")]
    mod in_memory_ {}
}

pub mod ext_codec {
    #[cfg(feature = "msgpack-rpc")]
    mod msgpack_rpc_ {}

    #[cfg(feature = "jsonrpc")]
    mod jsonrpc_ {}
}
