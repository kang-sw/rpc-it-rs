//! # Procedural macro for generating RPC signatures
//!
//! ## Attributes
//!
//! ```ignore
//! #[rpc_it::service]
//! #[service(name_prefix = "Namespace/", rename_all = "PascalCase")]
//! extern "module_name" {
//!   // If return type is specified explicitly, it is treated as request.
//!   fn method_req(arg: (i32, i32)) -> ();
//!
//!   // This is request; you can specify which error type will be returned.
//!   fn method_req_2<'a, 'borrow>() -> Result<MyParam<'borrow>, &'a str>;
//!
//!   // This is notification, which does not return anything.
//!   fn method_noti(arg: (i32, i32));
//!
//!   #[name = "MethodName"] // Client will encode the method name as this. Server takes either.
//!   #[route = "MyMethod/*"] // This will define additional route on server side
//!   #[route = "OtherMethodName"]
//!   fn method_example<'borrow>(arg: &'borrow str, arg2: &'borrow [u8])
//!
//!   // If serialization type and deserialization type is different, you can specify it by
//!   // double underscore and angle brackets, like specifying two parameters on generic type `__`
//!   fn from_to<'a>(s: __<i32, u64>, b: __<&'a str, String>) -> __<i32, String>;
//! }
//!
//! pub struct MyParam<'a> {
//!     name: &'a str,
//!     age: &'a str,
//! }
//! ```
//!

/*
    * TODO: Distinguish borrow and non-borrow types.

    mod module_name {
        #![allow(non_camel_case_types)]

        pub enum Inbound<U> {
            method_req(::rpc_it::macro::Request<U, self::method_req::Fn>),
            method_req_2(::rpc_it::macro::Request<U, self::method_req_2::Fn>),
            method_noti(::rpc_it::service::Notification<U, self::method_noti::Fn>),
        }

        impl<U> Inbound where U: UserData {
            pub fn route<F: Fn(Self) + Send + Sync + 'static>(
                router: &mut ::rpc_it::Router<U>,
                handler: impl Into<Arc<F>>,
            )
            {
                let handler = handler.into();

                {
                    let handler = handler.clone();

                    router.add_route("method_req", |inbound| {
                        unimplemented!("decode inbound and create 'Inbound' -> call handler");
                        handler(Self::message_req(unimplemented!()))
                    })
                }
            }
        }

        pub mod method_req {
            pub struct Fn;

            impl ::rpc_it::RequestMethod for method_req {
                type ParamSend<'a> = (i32, i32);
                type ParamRecv = (i32, i32);

                type ResultSend<'a> = ();
                type ResultRecv = ();
            }
        }

        pub mod method_req_2 {
            pub struct Fn;

            impl ::rpc_it::RequestMethod for method_req_2 {
                type ParamSend<'a> = ();
                type ParamRecv = ();

                type ResultSend<'a> = Result<MyParam<'a>, &'a str>;
                type ResultRecv = Result<Okay, Error>;
            }

            // Generate self-containing deserialized caches for each type that contains borrowed
            // lifetimes. Number of required lifetimes should be automatically counted from
            // function signature.

            pub struct Okay(::rpc_it::Bytes, MyParam<'static>);

            impl Okay {
                pub fn new(codec, payload) -> Self {
                    todo!("Decode payload, and transmute to elevate into static lifetime")
                }

                pub fn get<'__this>(&'__this self) -> &'__this MyParam<'__this> {
                    &self.1
                }
            }

            pub struct Error(::rpc_it::Bytes, &'static str);

            impl Error {
                pub fn new(codec, payload) -> Self {
                    todo!("Decode payload, and transmute to elevate into static lifetime")
                }

                pub fn get<'__this>(&'__this self) -> &'__this str {
                    &self.1
                }
            }
        }

        pub mod method_noti {
            pub struct Fn;

            impl ::rpc_it::NotifyMethod for method_noti {
                type ParamSend<'a> = (i32, i32);
                type ParamRecv = (i32, i32);
            }
        }
    }

    sender.req(module_name::method_req::Fn, &(1, 2));
    sender.try_req(module_name::method_req::Fn, &(1, 2));
    sender.noti(module_name::method_noti::Fn, &(1, 2)).await;
    sender.try_noti(module_name::method_noti::Fn, &(1, 2));
*/

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
    // dbg!(parsed);

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
