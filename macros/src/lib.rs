//! # Procedural macro for generating RPC signatures
//!
//! # Usage
//!
//! ```ignore
//! #[rpc_it::rpc]
//! mod my_api {
//! 	/// This defines request API. Return type can be specified.
//! 	#[api(request)]
//!     #[api(name = "Rpc.Call.MyMethod")]
//! 	pub fn my_method(my_binary: &[u8]) -> bool;
//!
//! 	/// This defines notification API. Return type must be `()`.
//! 	#[api(notify, recv_param = (String, _))]
//! 	pub fn my_notification(my_key: &str, my_value: u32);
//! }
//!
//! ```
//!
//! Above code will generate following APIs:
//!
//! - client side
//!
//! ``` ignore
//! let rpc: rpc_it::Sender = unimplemented!();
//!
//! async move {
//!   my_api::async_api::my_notification(rpc, "hey", 141).await;
//!
//!   match my_api::async_api::my_method(rpc, b"hello").await {
//!     Ok(x) => println!("my_method returned: {}", x),
//!     Err(e) => println!("my_method failed: {}", e),
//!   }
//! };
//! ```
//!
//! - Server side
//!
//! ``` ignore
//! let rpc: rpc_it::Transceiver = unimplemented!();
//! let mut service_builder: rpc_it::ServiceBuilder = unimplemented!();
//! let rx = my_api::register_service(&mut service_builder);
//!
//! async move {
//!   match rx.recv().await?.param {
//!     my_api::Inbound::MyMethod
//!   }
//!
//!   Ok::<(), anyhow::Error>(())
//! }
//! ```
//!
