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

pub use rpc::{
    msg::{Notify, RecvMsg, Request, Response},
    Builder, Channel, Client, Feature, InboundError, InboundEventSubscriber, Message, RecvError,
    RequestContext, ResponseFuture, SendError, TryRecvError,
};
