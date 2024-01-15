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
//! - `vis= "<VIS>"`
//!     - Visibility of generated methods. If not specified, it will be none.
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
//!   fn MethodReq(arg: (i32, i32)) -> ();
//!
//!   // This is request; you can specify which error type will be returned.
//!   fn MethodReq2() -> Result<MyParam<'_>, &'_ str>;
//!
//!   // This is notification, which does not return anything.
//!   fn MethodNotify(arg: (i32, i32));
//!
//!   #[name = "MethodName"] // Client will encode the method name as this. Server takes either.
//!   #[route = "MyMethod/*"] // This will define additional route on server side
//!   #[route = "OtherMethodName"]
//!   fn MethodExample(arg: &'_ str, arg2: &'_ [u8])
//!
//!   // If serialization type and deserialization type is different, you can specify it by
//!   // double underscore and angle brackets, like specifying two parameters on generic type `__`
//!   fn FromTo(s: __<i32, u64>, b: __<&'_ str, String>) -> __<i32, String>;
//! }
//!
//! pub struct MyParam<'a> {
//!     name: &'a str,
//!     age: &'a str,
//! }
//! ```
//!
//! ## Client side
//!
//! ```ignore
//!
//! let client: RequestSender = unimplemented!();
//!
//! client.try_call(module_name::method_req, )
//! ```
//!

use std::{borrow::Borrow, mem::take, sync::OnceLock};

use convert_case::Case;
use proc_macro_error::{abort, emit_error, proc_macro_error};

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::{
    punctuated::Punctuated, spanned::Spanned, ForeignItem, LitStr, Meta, PathSegment, Token,
};

macro_rules! ok_or {
    ($expr:expr) => {
        match $expr {
            Ok(expr) => Some(expr),
            Err(err) => {
                proc_macro_error::emit_call_site_error!("{}", err);
                None
            }
        }
    };

    ($span:expr, $expr:expr) => {
        match $expr {
            Ok(expr) => Some(expr),
            Err(err) => {
                proc_macro_error::emit_error!($span, "{}", err);
                None
            }
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

    /// Visibility of generated methods.
    vis: Option<syn::Visibility>,

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
                        self.name_prefix = expr_into_lit_str(meta.value);
                    } else if meta.path.is_ident("route_prefix") {
                        self.route_prefix = expr_into_lit_str(meta.value);
                    } else if meta.path.is_ident("vis") {
                        let Some(str) = expr_into_lit_str(meta.value) else {
                            continue;
                        };

                        let Some(vis) = ok_or!(str.parse::<syn::Visibility>()) else {
                            emit_error!(str, "Failed to parse visibility");
                            continue;
                        };

                        self.vis = Some(vis);
                    } else if meta.path.is_ident("rename_all") {
                        let Some(str) = expr_into_lit_str(meta.value) else {
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
        let mut vis_level = 0;

        if !self.is_flattened_module {
            vis_level += 1;

            let name_parse_result = item.abi.name.as_ref().unwrap().parse::<syn::Ident>();
            let Some(ident) = ok_or!(item.abi.name.span(), name_parse_result) else {
                abort!(
                    item.abi.name.span(),
                    "'{}' can't be made into valid identifier",
                    item.abi.name.as_ref().unwrap().value()
                );
            };

            let module_vis = self.vis.as_ref().unwrap_or(&syn::Visibility::Inherited);
            out.extend(quote!(#module_vis mod #ident))
        }

        let mut out_body = TokenStream::new();

        for item in item.items.into_iter() {
            match item {
                ForeignItem::Fn(x) => self.generate_item_fn(x, vis_level, &mut out_body),
                other => emit_error!(other, "Expected function declaration"),
            }
        }

        // TODO: Generate router for inbound requests/notifies

        if self.is_flattened_module {
            // Just expand the body as-is.
            out.extend(out_body);
        } else {
            // Wrap the result in block
            out.extend(quote!({ #out_body }))
        }
    }

    fn generate_item_fn(
        &mut self,
        item: syn::ForeignItemFn,
        vis_offset: usize,
        out: &mut TokenStream,
    ) {
        macro_rules! static_tok {
            ($($tok:tt)*) => {
                {
                    thread_local! {
                        static TOK: ::std::rc::Rc<TokenStream> = ::std::rc::Rc::new(quote!($($tok)*));
                    }

                    TOK.with(|tok| tok.clone())
                }
            };
        }

        let mod_macros = static_tok!(::rpc_it::macros);

        /*
            <elevated_vis> mod <method_name> {
                #![allow(non_camel_case_types)]

                struct Method;

                impl NotifyMethod for Method {
                    type ParamSend<'a> = ..;
                    type ParamRecv<'a> = ..;
                }

                impl RequestMethod for Method {
                    ..
                }
            }

            const <method_name>: <method_name>::Method = <method_name>::Method;
        */

        // 1. Analyze visibility and name

        // 2. Generate method definition

        // 3. Analyze input types; generate `impl NotifyMethod`

        // 4. Analyze output types; generate `impl RequestMethod`
    }
}

fn elevate_vis_level(mut in_vis: syn::Visibility, amount: usize) -> syn::Visibility {
    // - TODO: pub(super) -> pub(super::super)
    // - TODO: None -> pub(super)
    // - TODO: pub(in crate::...) -> absolute; as is
    // - TODO: pub(in super::...) -> pub(in super::super::...)

    if amount == 0 {
        return in_vis;
    }

    match in_vis {
        syn::Visibility::Public(_) => in_vis,
        syn::Visibility::Restricted(ref mut vis) => {
            let first_ident = &vis.path.segments.first().unwrap().ident;

            if first_ident == "crate" {
                // pub(in crate::...) -> Don't need to elevate
                return in_vis;
            }

            if vis.in_token.is_some() && vis.path.leading_colon.is_some() {
                // Absolute path
                return in_vis;
            }

            let is_first_token_self = first_ident == "self";
            let source = take(&mut vis.path.segments);
            vis.path.segments.extend(
                std::iter::repeat(PathSegment {
                    arguments: syn::PathArguments::None,
                    ident: syn::Ident::new("super", vis.span()),
                })
                .take(amount),
            );
            vis.path
                .segments
                .extend(source.into_iter().skip(is_first_token_self as usize));

            in_vis
        }
        syn::Visibility::Inherited => {
            let span = || in_vis.span();
            syn::Visibility::Restricted(syn::VisRestricted {
                pub_token: Token![pub](span()),
                paren_token: syn::token::Paren(span()),
                in_token: Some(Token![in](span())),
                path: Box::new(syn::Path {
                    leading_colon: None,
                    segments: std::iter::repeat(syn::PathSegment {
                        arguments: syn::PathArguments::None,
                        ident: syn::Ident::new("super", span()),
                    })
                    .take(amount)
                    .collect(),
                }),
            })
        }
    }
}

fn expr_into_lit_str(expr: syn::Expr) -> Option<syn::LitStr> {
    match expr {
        syn::Expr::Lit(expr) => match expr.lit {
            syn::Lit::Str(lit) => return Some(lit),
            _ => proc_macro_error::emit_error!(expr, "Expected string literal"),
        },
        _ => proc_macro_error::emit_error!(expr, "Expected string literal"),
    }

    None
}
