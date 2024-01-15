//! # Procedural macro for generating RPC signatures
//!
//! ## Concepts
//!
//! - **Method Name**: Name of the method. The sender uses this name when serializing the request.
//!   The receiver uses this name to find the handler, using 'exact match' strategy. This is the
//!   very default configuration to filter inbound requests/notifies.
//! - **Method Route**: When creating server side's method router, you can specify additional route
//!   configurations for each method. This is useful when you want to create a method that can be
//!   called with multiple method names/patterns(if router supports it). This is not used for
//!   outbound requests/notifies.
//! - **Request Method**: A method that returns a value. The sender will wait for the response.
//! - **Notification Method**: A method that does not return a value. This is work as
//!   fire-and-forget method; which you even don't know if the receiver received the request.
//!
//! ## Attributes
//!
//! ### `#[rpc_it::service(<ATTRS>)]`
//!
//! - `flatten`
//!     - If specified, the generated code will be flattened into the current module.
//! - `name_prefix = "<PREFIX>"`
//!     - Every method names will be prefixed with given string.
//! - `route_prefix = "<PREFIX>"`
//!     - Every method routes will be prefixed with given string.
//! - `rename_all = "<CASE>"`
//!     - Every method names will be renamed with given case.
//!     - Supported case conventions are `snake_case`, `camelCase`, `PascalCase`,
//!       `SCREAMING_SNAKE_CASE`, and `kebab-case`. Which follows similar rule with
//!       `serde(rename_all = "<CASE>")`.
//!
//! ### Method Attributes
//!
//! - `[name = "<NAME>"]`
//!     - The method name will be renamed with given string.
//! - `[route = "<ROUTE>"]`
//!     - Adds additional route to the method.
//!
//! ## Usage
//!
//! ```ignore
//! #[rpc_it::service(name_prefix = "Namespace/", rename_all = "PascalCase")]
//! extern "module_name" {
//!   // If return type is specified explicitly, it is treated as request.
//!   fn method_req(arg: (i32, i32)) -> ();
//!
//!   // This is request; you can specify which error type will be returned.
//!   fn method_req_2() -> Result<MyParam<'_>, &'_ str>;
//!
//!   // This is notification, which does not return anything.
//!   fn method_noti(arg: (i32, i32));
//!
//!   #[name = "MethodName"] // Client will encode the method name as this. Server takes either.
//!   #[route = "MyMethod/*"] // This will define additional route on server side
//!   #[route = "OtherMethodName"]
//!   fn method_example(arg: &'_ str, arg2: &'_ [u8])
//!
//!   // If serialization type and deserialization type is different, you can specify it by
//!   // double underscore and angle brackets, like specifying two parameters on generic type `__`
//!   fn from_to(s: __<i32, u64>, b: __<&'_ str, String>) -> __<i32, String>;
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
        #![allow(unused)]

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

use convert_case::Case;
use proc_macro_error::proc_macro_error;

use proc_macro2::TokenStream;
use syn::{punctuated::Punctuated, spanned::Spanned, Meta, Token};

macro_rules! unwrap_result {
    ($expr:expr) => {
        match $expr {
            Ok(expr) => expr,
            Err(err) => proc_macro_error::abort_call_site!("{}", err),
        }
    };

    ($span:expr, $expr:expr) => {
        match $expr {
            Ok(expr) => expr,
            Err(err) => proc_macro_error::abort!($span, "{}", err),
        }
    };
}

#[proc_macro_error]
#[proc_macro_attribute]
pub fn service(
    attr: proc_macro::TokenStream,
    items: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // let Ok(item) = syn::parse::<syn::ItemForeignMod>(items.clone()) else {
    //     proc_macro_error::emit_call_site_error!("Expected foreign module");
    //     return items;
    // };
    let item = syn::parse_macro_input!(items as syn::ItemForeignMod);

    let mut model = DataModel::default();
    let mut out_stream = TokenStream::new();

    model.parse_attr(attr.into());
    model.main(item, &mut out_stream);

    out_stream.into()
}

/// A model which describes parsed declarations
#[derive(Default)]
struct DataModel {
    /// If the descriptions are being generated within current module. All visibility specifications
    /// will be reduced by one level.
    is_flattened_module: bool,

    /// Prefix for all method names.
    name_prefix: Option<syn::LitStr>,

    /// Prefix for all method routes.
    route_prefix: Option<syn::LitStr>,

    /// Rename all method names with given case.
    rename_all: Option<Case>,

    /// Notify method definitions. Used for generating inbound router.
    notifies: Vec<MethodDef>,

    /// Request method definitions. Used for generating inbound router.
    requests: Vec<MethodDef>,
}

struct MethodDef {
    vis: syn::Visibility,
    ident: syn::Ident,
    name: Option<syn::LitStr>,
    routes: Vec<syn::LitStr>,
}

impl DataModel {
    fn parse_attr(&mut self, attrs: proc_macro2::TokenStream) {
        // TODO:
        let attrs: syn::Meta = syn::parse_quote_spanned! { attrs.span() => service(#attrs) };
        let syn::Meta::List(attrs) = attrs else {
            return;
        };

        let attrs = match attrs.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
            Ok(attrs) => attrs,
            Err(err) => {
                proc_macro_error::emit_error!("{}", err);
                return; // Just skip attribute parsing
            }
        };

        for meta in attrs.into_iter() {
            let mut err_ident = None;

            match meta {
                Meta::Path(meta) => {
                    if meta.is_ident("flatten") {
                        self.is_flattened_module = true;
                    } else {
                        err_ident = meta.get_ident().cloned();
                    }
                }
                Meta::NameValue(meta) => {
                    if meta.path.is_ident("name_prefix") {
                        self.name_prefix = expr_unwrap_lit_str(meta.value);
                    } else if meta.path.is_ident("route_prefix") {
                        self.route_prefix = expr_unwrap_lit_str(meta.value);
                    } else if meta.path.is_ident("rename_all") {
                        let Some(str) = expr_unwrap_lit_str(meta.value) else {
                            continue;
                        };

                        match str.value().as_str() {
                            "snake_case" => self.rename_all = Some(Case::Snake),
                            "camelCase" => self.rename_all = Some(Case::Camel),
                            "PascalCase" => self.rename_all = Some(Case::Pascal),
                            "SCREAMING_SNAKE_CASE" => self.rename_all = Some(Case::ScreamingSnake),
                            "kebab-case" => self.rename_all = Some(Case::Kebab),
                            "UPPERCASE" => self.rename_all = Some(Case::Upper),
                            "lowercase" => self.rename_all = Some(Case::Lower),
                            _ => {
                                proc_macro_error::emit_error!(
                                    str,
                                    "Unknown case convention '{}', case must be one of \
                                     'snake_case', 'camelCase', 'PascalCase', \
                                     'SCREAMING_SNAKE_CASE', 'kebab-case', 'UPPERCASE', \
                                     'lowercase'",
                                    str.value()
                                );
                            }
                        }
                    } else {
                        err_ident = meta.path.get_ident().cloned();
                    }
                }
                Meta::List(m) => {
                    err_ident = m.path.get_ident().cloned();
                }
            }

            if let Some(ident) = err_ident {
                proc_macro_error::emit_error!(
                    ident,
                    "Unexpected or incorrect usage of attribute argument '{}'",
                    ident
                );
            }
        }
    }

    fn main(&mut self, item: syn::ItemForeignMod, out: &mut TokenStream) {
        // TODO:
    }
}

fn expr_unwrap_lit_str(expr: syn::Expr) -> Option<syn::LitStr> {
    match expr {
        syn::Expr::Lit(expr) => match expr.lit {
            syn::Lit::Str(lit) => return Some(lit),
            _ => proc_macro_error::emit_error!(expr, "Expected string literal"),
        },
        _ => proc_macro_error::emit_error!(expr, "Expected string literal"),
    }

    None
}
