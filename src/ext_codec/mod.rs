//! Implementations of RPC codecs ([`Codec`]) for various protocols

#[cfg(feature = "jsonrpc")]
pub mod jsonrpc;
#[cfg(feature = "msgpack-rpc")]
pub mod msgpackrpc {}
#[cfg(feature = "mspack-rpc-postcard")]
pub mod msgpackrpc_postcard {}
