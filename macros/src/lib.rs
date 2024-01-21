//! # rpc-it-macros
//!
//! `rpc-it-macros` is a Rust utility crate designed to significantly enhance the development
//! experience when working with RPC (Remote Procedure Call) systems. This crate primarily focuses
//! on RPC code generation, leveraging Rust's strong type system.
//!
//! ## What Does This Library Do?
//!
//! The core functionality of `rpc-it-macros` lies in its ability to automate the generation of
//! RPC-related code. By utilizing Rust's type system, this crate ensures that the code for handling
//! RPC calls is generated in a way that is both type-safe and efficient. This approach minimizes
//! the boilerplate code typically associated with setting up RPCs, leading to a cleaner and more
//! maintainable codebase.
//!
//! ## Why Do You Need This?
//!
//! In the world of software development, especially when dealing with inter-process or network
//! communication, minimizing human error is crucial. `rpc-it-macros` addresses this by offering a
//! code-driven approach to RPC. This method reduces the likelihood of errors that can arise from
//! manual setup and maintenance of RPC calls and routes. By integrating this crate into your
//! project, you ensure that your RPC implementations are not only correct by design but also
//! consistent and reliable.
//!
//! ## Getting Started
//!
//! To integrate `rpc-it-macros` into your Rust project, add it as a dependency in your `Cargo.toml`
//! file:
//!
//! ```toml
//! [dependencies]
//! rpc-it-macros = "0.10.0"
//! ```
//!
//! > Disclaimer: This README was generated with the assistance of AI. If there are any conceptual
//! > errors or areas of improvement, please feel free to open an issue on our repository. Your
//! > feedback is invaluable in enhancing the accuracy and utility of this documentation.
//!

use convert_case::Case;
use proc_macro_error::proc_macro_error;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{punctuated::Punctuated, spanned::Spanned, Item, Meta, Token};

mod type_util;

/// # Procedural Macro for Generating RPC Signatures
///
/// This documentation explains the procedural macro used for generating RPC signatures, focusing on
/// key concepts and attributes.
///
/// ## Key Concepts
///
/// - **Method Name** is the identifier for a method. Senders use this name to serialize requests,
///   while receivers use it to locate the appropriate handler through an 'exact match' strategy.
///   This is the default approach for filtering inbound requests and notifications.
///
/// - **Method Route**, in server-side method routing, you can specify additional routing
///   configurations for each method. This is useful for creating methods that respond to multiple
///   names or patterns (if supported by the router). Method routes are not applicable to outbound
///   requests or notifications.
///
/// - **Request Method** is a method that returns a value. The sender waits for a response from this
///   type of method.
///
/// - **Notification Method** is a method that does not return a value. It operates on a
///   'fire-and-forget' basis, meaning the sender has no confirmation of the receiver having
///   received the request.
///
/// ## Attributes
///
/// ### Service Attribute: `#[rpc_it::service(<ATTRS>)]`
///
/// - `name_prefix = "<PREFIX>"`
///     - Appends a specified prefix to every method name.
/// - `route_prefix = "<PREFIX>"`
///     - Appends a specified prefix to every method route.
/// - `rename_all = "<CASE>"`
///     - Renames all default method names according to the specified case convention.
///         - Explicit renamings using `#[name = "<NAME>"]` are exempt from this rule.
///     - Supported case conventions: `snake_case`, `camelCase`, `PascalCase`,
///       `SCREAMING_SNAKE_CASE`, `kebab-case`. The convention follows rules similar to
///       `serde(rename_all = "<CASE>")`.
/// - `vis = "<VIS>"`
///     - Sets the visibility of generated methods. Defaults to private if unspecified.
/// - `handler_module_name = "<NAME>"`
///     - Sets the name of the module containing the generated handler. Defaults to `handler`.
/// - `no_recv`
///     - Do not generate receiver part of the module.
/// - `no_param_recv_newtype`
///     - Do not generate new type for `ParamRecv` types. This will reduce generated code size.
///
/// ### Method Attributes
///
/// - `[name = "<NAME>"]`
///     - Renames the method to the specified string.
/// - `[route = "<ROUTE>"]`
///     - Adds an additional route to the method.
/// - `[no_recv]`
///     - Do not define handler for this method.
///
/// ### Router Attributes
///
/// - `[no_default_route]`
///     - Do not generate default route for methods listed in this router.
/// - `[install]`
///     - Generate `install` function
/// - `[direct]`
///     - Generate `direct` mode router for this method.
///
/// ## Serialization / Deserialization Rules
///
/// - Single-argument methods: The argument is serialized as-is.
/// - Multi-argument methods: Arguments are serialized as a tuple (array in most serialization
///   formats).
///     - To serialize a single argument as a tuple, wrap it in an additional tuple (e.g., `T`
///       becomes `(T,)`).
///
/// ## Usage
///
/// ```ignore
/// #[rpc_it::service(name_prefix = "Namespace/", rename_all = "PascalCase")]
/// pub mod my_service {
///   // If return type is specified explicitly, it is treated as request.
///   pub fn method_req(arg: (i32, i32)) -> ();
///
///   // This is request; you can specify which error type will be returned.
///   pub fn method_req_2() -> Result<MyParam<'_>, &'_ str> ;
///
///   // This is notification, which does not return anything.
///   pub fn method_notify(arg: (i32, i32)) {}
///
///   #[name = "MethodName"] // Client will encode the method name as this. Server takes either.
///   #[route = "MyMethod/*"] // This will define additional route on server side
///   #[route = "OtherMethodName"]
///   pub(crate) fn method_example(arg: &'_ str, arg2: &'_ [u8]) ;
///
///   // If serialization type and deserialization type is different, you can specify it by
///   // double underscore and angle brackets, like specifying two parameters on generic type `__`
///   //
///   // It may work incorrectly if the underlying codec is not self-descriptive format such as
///   // postcard.
///   fn from_to(s: __<i32, u64>, b: __<&'_ str, String>) -> __<i32, String> ;
///
///   // After defining all methods, you can define server-side router by defining `Route` const
///   // item. `ALL` and `ALL_PASCAL_CASE` generates router for every method within this namespace,
///   // `ALL` will use method name as variant identifier as-is, while `ALL_PASCAL_CASE` will
///   // convert method name to `PascalCase`.
///   const MyRouterIdent: Route = ALL;
///
///   // You can also choose list of routed methods selectively:
///   #[install]
///   #[direct]
///   const MySelectiveRoute: Route = [
///     /// (Documentation comment is allowed)
///     ///
///     /// A identifier to rpc method must be specified here.
///     method_example,
///
///     /// By opening braces next to method identifier, you can specify additional route
///     /// specs for this method.
///     from_to {
///         routes = ["aaabbb", "from_to2"],
///     },
///
///     /// You can route same method repeatedly, with different route config.
///     /// (In practice, this is pointless)
///     ///
///     /// In following declaration, right side of the identifier is treated as new variant name.
///     from_to = VariantName {
///         no_default_route, // Prevents error from duplicated route
///         routes = ["OtherRoute", "AAaaaH"]
///     }
///   ];
///
/// }
///
/// pub struct MyParam<'a> {
///     name: &'a str,
///     age: &'a str,
/// }
/// ```
///
/// ## Client side
///
/// ```ignore
///
/// let client: RequestSender = unimplemented!();
///
/// client.try_call(your_module_name::method_req, )
/// ```
///
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
    let item = syn::parse_macro_input!(items as syn::ItemMod);

    let mut model = DataModel::default();
    let mut out_stream = TokenStream::new();

    model.parse_attr(attr.into());
    model.main(item, &mut out_stream);

    out_stream.into()
}

/// A model which describes parsed declarations
#[derive(Default)]
struct DataModel {
    /// Prefix for all method names.
    name_prefix: Option<syn::LitStr>,

    /// Prefix for all method routes.
    route_prefix: Option<syn::LitStr>,

    /// Rename all method names with given case.
    rename_all: Option<Case>,

    /// No receiver part of the module.
    no_recv: bool,

    /// No newtype for `ParamRecv` types.
    no_param_recv_newtype: bool,

    /// Visibility of generated methods.
    vis: Option<syn::Visibility>,

    /// Notify method definitions. Used for generating inbound router.
    handled_methods: Vec<MethodDef>,
}

#[derive(Clone)]
struct MethodDef {
    is_req: bool,
    method_ident: syn::Ident,
    name: String,
    routes: Vec<syn::LitStr>,
}

impl DataModel {
    fn vis(&self) -> &syn::Visibility {
        self.vis.as_ref().unwrap_or(&syn::Visibility::Inherited)
    }
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
                    if meta.is_ident("no_param_recv_newtype") {
                        self.no_param_recv_newtype = true;
                    } else if meta.is_ident("no_recv") {
                        self.no_recv = true;
                    } else {
                        err_ident = meta.get_ident().cloned();
                    }
                }
                Meta::NameValue(meta) => {
                    if meta.path.is_ident("name_prefix") {
                        self.name_prefix = type_util::expr_into_lit_str(meta.value);
                    } else if meta.path.is_ident("route_prefix") {
                        self.route_prefix = type_util::expr_into_lit_str(meta.value);
                    } else if meta.path.is_ident("rename_all") {
                        let Some(str) = type_util::expr_into_lit_str(meta.value) else {
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

    fn main(&mut self, item: syn::ItemMod, out: &mut TokenStream) {
        let vis_level = 1;

        // TODO: Find way to use re-imported crate name
        let crate_name = quote!(rpc_it);

        // Create module name
        {
            self.vis = Some(item.vis);
            let vis = self.vis();
            let ident = &item.ident;

            out.extend(quote!(#vis mod #ident));
        }

        let mut out_method_defs = TokenStream::new();
        let mut out_routes = TokenStream::new();
        let mut pass_thru_items = Vec::new();
        let mut route_items = Vec::new();

        // Parse list of module items
        // - If it's function body (or verbatim function declaration), generate method definition
        // - If it's const item, try parse it as route item
        for item in item.content.unwrap().1.into_iter() {
            let pass_thru = match item {
                Item::Fn(syn::ItemFn {
                    attrs,
                    vis,
                    sig,
                    block: _,
                }) => {
                    self.generate_item_fn(
                        syn::ForeignItemFn {
                            semi_token: Token![;](sig.span()),
                            attrs,
                            sig,
                            vis,
                        },
                        vis_level,
                        &mut out_method_defs,
                    );
                    continue;
                }
                Item::Const(item) => {
                    if let Some(path_seg) = type_util::type_path_as_mono_seg(&item.ty) {
                        if path_seg.ident == "Route" {
                            route_items.push(item);
                            continue;
                        }
                    };
                    Item::Const(item)
                }
                Item::Verbatim(verbatim) => {
                    if let Ok(x @ syn::ItemForeignMod { .. }) =
                        syn::parse2(quote!(extern "C" { #verbatim }))
                    {
                        if let syn::ForeignItem::Fn(item) = x.items.into_iter().next().unwrap() {
                            self.generate_item_fn(item, vis_level, &mut out_method_defs);
                            continue;
                        }
                    }

                    Item::Verbatim(verbatim)
                }
                other => other,
            };

            pass_thru_items.push(pass_thru);
        }

        for item in route_items {
            self.generate_route(item, vis_level, &mut out_routes);
        }

        // Wrap the result in block
        out.extend(quote!({
            #![allow(unused_parens)]
            #![allow(unused)]
            #![allow(non_camel_case_types)]
            #![allow(non_snake_case)]
            #![allow(clippy::needless_lifetimes)]

            use super::*; // Make transparent to parent module

            use #crate_name as ___crate;

            use ___crate::serde as serde;

            use ___crate::macros as ___macros;
            use ___macros::route as ___route;

            // Pass through all other items
            #(#pass_thru_items)*

            // Let routes appear before methods; let method decls appear as method syntax by
            // language server
            #out_routes

            // Method definitions here.
            #out_method_defs
        }))
    }

    fn find_method(&self, ident: &syn::Ident) -> Option<&MethodDef> {
        self.handled_methods
            .iter()
            .find(|x| &x.method_ident == ident)
    }

    fn find_method_by_path(&self, path: &syn::Path) -> Option<&MethodDef> {
        let seg = type_util::path_as_mono_seg(path)?;
        self.find_method(&seg.ident)
    }
}

mod gen_route;

mod gen_func;
