//! # Procedural macro for generating RPC signatures
//!
//! ## Attributes
//!
//! - `notify`, `request`: Determines whether the API is a notification or a request.
//! - `name`: Specifies the API name. If not specified, the name will be generated from the function
//!   name.
//! - `deserialize_args_as`: Overrides the deserialization type of the arguments.
//! - `serialize_return_as`: Overrides the serialization type of the return value. Only valid for
//!  request APIs.
//!
//! # Usage
//!
//! ```ignore
//! #[rpc_it::service]
//! mod my_api {
//!     /// This defines request API. Return type can be specified.
//!     #[request]
//!     #[api(name = "Rpc.Call.MyMethod")]
//!     pub fn my_method<'a>(my_binary: &'a [u8]) -> bool;
//!
//!     /// This defines notification API. Return type must be `()`.
//!     #[notify]
//!     #[api(deserialize_args = (String, _))]
//!     pub fn my_notification(my_key: &str, my_value: u32);
//! }
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
//! let (tx, rx) = flume::unbounded();
//!
//! //
//! my_api::register_service(&mut service_builder, |inbound: my_api::Inbound| {
//!   match inbound {
//!     // Allows 'take' out owned deserialized data, as data movement is inherently prohibited.
//!     my_api::Param::MyMethod(req) => {
//!       // req: my_api::de::MyMethod
//!     }
//!     my_api::Param::MyNotification(noti) => {
//!       // noti: my_api::de::MyNotification
//!     }
//!   }
//! });
//! ```
//!
//! - Generation example
//!
//! ```ignore
//! mod my_api {
//!   mod param {
//!     struct MyMethod {
//!       __h: rpc_it::Sender,
//!       __id: rpc_it::ReqId,
//!       __p: Bytes, // payload segment
//!     }
//!
//!     impl rpc_it::ExtractUserData for MyMethod {
//!        // ...
//!     }
//!
//!     impl MyMethod {
//!       pub fn parse<'this>(&'this self) -> Result<super::de::MyMethod<'this>, rpc_it::Error> {
//!         // ...
//!       }
//!
//!       pub fn respond(self) -> rpc_it::TypedRequest<bool> {
//!         // ...
//!       }
//!     }
//!   }
//!   mod de {
//!     #[derive(serde::Deserialize)]
//!     struct MyMethod<'a> {
//!       pub my_binary: &'a [u8],
//!     }
//!   }
//! }
//! ```
//!

use proc_macro_error::proc_macro_error;
use quote::quote;

#[proc_macro_error]
#[proc_macro_attribute]
pub fn service(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // Replace first 'mod' into 'trait'
    let mut is_trait_now = false;
    let tokens = item.into_iter().map(|token| {
        use proc_macro::*;

        if is_trait_now {
            token
        } else {
            if let TokenTree::Ident(ident) = &token {
                match ident.to_string().as_str() {
                    "mod" => {
                        is_trait_now = true;
                        return TokenTree::Ident(Ident::new("trait", ident.span()));
                    }
                    "trait" => {
                        is_trait_now = true;
                        return token;
                    }
                    _ => {}
                }
            }

            token
        }
    });
    let mut item = proc_macro::TokenStream::new();
    item.extend(tokens);

    // Parse module definition as trait definition, to parse function declarations as valid rust
    // syntax.
    let parsed = syn::parse::<syn::Item>(item).unwrap();
    dbg!(parsed);

    quote! {
        fn fewfew() {}
    }
    .into()
}

struct SharedConfig {}

struct MethodDefinition {}

enum MethodDetail {
    Request { name: String },
}
