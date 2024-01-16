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

use convert_case::{Case, Casing};
use proc_macro_error::{abort, emit_error, proc_macro_error};

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::{punctuated::Punctuated, spanned::Spanned, GenericArgument, Item, Meta, Token, Type};

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
///   fn from_to(s: __<i32, u64>, b: __<&'_ str, String>) -> __<i32, String> ;
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
    routes: Vec<String>,
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

        // TODO: In future, find re-imported crate name
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
        let mut route_items = Vec::new();

        for item in item.content.unwrap().1.into_iter() {
            match item {
                Item::Fn(syn::ItemFn {
                    attrs,
                    vis,
                    sig,
                    block: _,
                }) => self.generate_item_fn(
                    syn::ForeignItemFn {
                        semi_token: Token![;](sig.span()),
                        attrs,
                        sig,
                        vis,
                    },
                    vis_level,
                    &mut out_method_defs,
                ),
                Item::Const(item) => {
                    let Some(path_seg) = type_util::type_path_as_mono_seg(&item.ty) else {
                        emit_error!(item.ty, "Expected single type path segment");
                        continue;
                    };

                    if path_seg.ident == "Route" {
                        route_items.push(item);
                    } else {
                        // NOTE: Update this on every route item addition
                        emit_error!(item.ty, "Unknown type. Expected 'Route'");
                    }
                }
                Item::Verbatim(verbatim) => {
                    let Ok(x @ syn::ItemForeignMod { .. }) =
                        syn::parse2(quote!(extern "C" { #verbatim }))
                    else {
                        continue;
                    };

                    let Some(syn::ForeignItem::Fn(item)) = x.items.into_iter().next() else {
                        continue;
                    };

                    self.generate_item_fn(item, vis_level, &mut out_method_defs);
                }
                other => emit_error!(other, "Unknown item type"),
            }
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

    fn generate_route(&mut self, item: syn::ItemConst, vis_offset: usize, out: &mut TokenStream) {
        struct GenDesc {
            variant_ident: Option<syn::Ident>,
            def: MethodDef,
        }

        impl GenDesc {
            fn new(def: MethodDef) -> Self {
                Self {
                    variant_ident: None,
                    def,
                }
            }
        }

        let mut generate_targets = Vec::<GenDesc>::new();

        // -- Retrieve method definitions to generate

        // If expr is `= ALL` then generate all methods. Otherwise, generate only specified methods.
        // This shoud be `[<<method_name>>, <<alias>> = <<method_name>>, ...]`
        match &*item.expr {
            syn::Expr::Path(syn::ExprPath {
                path, qself: None, ..
            }) if path.is_ident("ALL") => {
                generate_targets.extend(self.handled_methods.iter().cloned().map(GenDesc::new));
            }

            syn::Expr::Path(syn::ExprPath {
                path, qself: None, ..
            }) if path.is_ident("ALL_PASCAL_CASE") => {
                generate_targets.extend(self.handled_methods.iter().map(|x| GenDesc {
                    variant_ident: {
                        let str = x.method_ident.to_string().to_case(Case::Pascal);
                        Some(syn::Ident::new(&str, x.method_ident.span()))
                    },
                    def: x.clone(),
                }));
            }

            syn::Expr::Array(syn::ExprArray { elems, .. }) => {
                // Rules:
                // - <<method_name>>
                // - <<method_name>> { <<rules>> }
                // - <<method_name>> = <<rename>> { <<rules>> }

                for elem in elems {
                    match elem {
                        syn::Expr::Path(syn::ExprPath {
                            qself: None, path, ..
                        }) => {
                            if let Some(def) = self.find_method_by_path(path) {
                                generate_targets.push(GenDesc::new(def.clone()));
                            } else {
                                emit_error!(path, "Unknown method");
                            }
                        }

                        syn::Expr::Assign(_) => todo!(),

                        syn::Expr::Struct(_) => todo!(),

                        elem => {
                            emit_error!(
                                elem,
                                "Expected path, struct or assignment style declaration"
                            );
                        }
                    }
                }
            }

            other => {
                emit_error!(
                    other,
                    "Expected 'ALL|ALL_PASCAL_CASE' or list of method names"
                );
            }
        }

        // -- Generate route definitions

        /*  NOTE

            <<vis>> enum <<route_ident_name>> <U: ___crate::UserData> {
                <<variant_ident>>(
                    ___crate::cached::Cached<<Request|Notify>><U, <<method_name>>::Fn>
                ),
            }

            impl<U: ___crate::UserData> <<route_ident_name>> <U> {
                /// # Panics
                ///
                /// Router installation failed.
                <<vis>> fn install<F>(
                    ___router: &mut ___route::RouterBuilder<U, F>,
                    ___handler: impl Fn(Self) + Send + Sync + 'static + Clone,
                )
                    where F: ___route::RouterFuncBuilder
                {
                    // <<method_name>>
                    {
                        let ___handler = ___handler.clone();
                        ___router.push_handler(move |___inbound| {
                            let ___ib = ___inbound.take();

                            ___handler(
                                ___crate::cached::Cached<<Request|Notify>>::__internal_create(
                                    inbound.into_owned()
                                ).map_err(|(___ib, ___err)| {
                                    ___inbound = ___ib;
                                    ___err.into()
                                })
                            );

                            Ok(())
                        });
                    }

                }
            }
        */

        let vis_this = type_util::elevate_vis_level(item.vis.clone(), vis_offset);
        let ident_this = &item.ident;

        let tok_enum_title = { quote!(#vis_this enum #ident_this) };

        let tok_enum_variants = if generate_targets.is_empty() {
            quote!(__EmptyVariant(::std::marker::PhantomData<U>))
        } else {
            let tokens = generate_targets.iter().map(
                |GenDesc {
                     variant_ident,
                     def:
                         MethodDef {
                             is_req,
                             method_ident,
                             ..
                         },
                 }| {
                    let ident = variant_ident.as_ref().unwrap_or(method_ident);
                    let type_path = if *is_req {
                        quote!(Request)
                    } else {
                        quote!(Notify)
                    };

                    quote!(
                        #ident(___crate::cached:: #type_path <U, self:: #method_ident :: Fn>)
                    )
                },
            );

            quote!(#(#tokens),*)
        };

        let handler_impl_clone = (generate_targets.len() > 1).then(|| quote!(+ Clone));
        let tok_install_contents = generate_targets.iter().enumerate().map(
            |(
                index,
                GenDesc {
                    variant_ident,
                    def:
                        MethodDef {
                            is_req,
                            method_ident,
                            name,
                            routes,
                        },
                },
            )| {
                let ident = variant_ident.as_ref().unwrap_or(method_ident);
                let type_path = if *is_req {
                    quote!(Request)
                } else {
                    quote!(Notify)
                };

                let route_strs = Some(name)
                    .into_iter()
                    .chain(routes.iter())
                    .map(|x| quote!(#x));

                let is_last = index == generate_targets.len() - 1;
                let clone_handler = (!is_last).then(|| quote!(.clone()));
                let method_name_str = method_ident.to_string();

                quote!({
                    let ___handler = ___handler #clone_handler;

                    ___router.push_handler(move |___inbound| {
                        let ___ib = ___inbound.take().unwrap();

                        #[allow(unsafe_code)]
                        unsafe {
                            ___handler(
                                #ident_this :: #ident (
                                    ___crate::cached:: #type_path ::__internal_create(
                                        ___ib.into_owned(),
                                    ).map_err(|(___ib, ___err)| {
                                        *___inbound = Some(___ib);
                                        ___err
                                    })?
                                )
                            );
                        }

                        Ok(())
                    });

                    let ___routes = [#(#route_strs),*];
                    for ___route in ___routes {
                        let Err(___err) = ___router.try_add_route_to_last(___route) else {
                            continue;
                        };

                        match ___on_route_error(#method_name_str, ___route, ___err).into() {
                            ___RF::IgnoreAndContinue => {}
                            ___RF::Panic => {
                                panic!(
                                    "Failed to add route '{}::{}'",
                                    #method_name_str,
                                    ___route
                                );
                            },
                            ___RF::Abort => return,
                        }
                    }
                })
            },
        );

        out.extend(quote!(
            #tok_enum_title<U: ___crate::UserData> {
                #tok_enum_variants
            }

            impl<U: ___crate::UserData> #ident_this<U> {
                #vis_this fn install<B, E>(
                    ___router: &mut ___route::RouterBuilder<U, B>,
                    ___handler: impl Fn(Self) + Send + Sync + 'static #handler_impl_clone,
                    mut ___on_route_error: impl FnMut(&str, &str, B::Error) -> E,
                ) where
                    B: ___route::RouterFuncBuilder,
                    E: Into<___route::RouteFailResponse>,
                {
                    use ___route::RouteFailResponse as ___RF;



                    #(#tok_install_contents)*
                }
            }
        ))
    }

    fn generate_item_fn(
        &mut self,
        item: syn::ForeignItemFn,
        vis_offset: usize,
        out: &mut TokenStream,
    ) {
        /*  NOTE

            <elevated_vis> mod <method_name> {
                #![allow(non_camel_case_types)]

                struct Fn;

                impl NotifyMethod for Fn {
                    type ParamSend<'a> = ..;
                    type ParamRecv<'a> = ..;
                }

                impl RequestMethod for Fn {
                    ..
                }
            }

            <elevated_vis> fn <method_name>(<ser_params>...) -> (
                <method_name>::Fn,
                <method_name>::ParamSend<'_>,
            ) {
                (
                    <method_name>::Fn,
                    <ser_params>...,
                )
            }
        */

        let item_span = item.span();

        let vis_outer = type_util::elevate_vis_level(item.vis, vis_offset);
        let vis_inner = type_util::elevate_vis_level(vis_outer.clone(), 1);

        let mut attrs = TokenStream::new();
        let mut no_recv = false;

        let mut def = MethodDef {
            is_req: false,
            name: Default::default(),
            method_ident: item.sig.ident,
            routes: Vec::new(),
        };

        if let Some(case) = &self.rename_all {
            def.name = convert_case::Casing::to_case(&def.name, *case);
        }

        'outer: for mut attr in item.attrs {
            attr.meta = match attr.meta {
                Meta::Path(path) if path.is_ident("no_recv") => {
                    no_recv = true;
                    continue 'outer;
                }

                // If it's doc name-value, just leave it as-is
                Meta::NameValue(meta) if meta.path.is_ident("doc") => Meta::NameValue(meta),

                // Match name-value attributes
                Meta::NameValue(meta) => 'value: {
                    if meta.path.is_ident("name") {
                        if !def.name.is_empty() {
                            emit_error!(meta, "Duplicated name attribute");
                            continue 'outer;
                        }

                        let Some(lit) = type_util::expr_into_lit_str(meta.value) else {
                            emit_error!(meta.path, "'name' must be string literal");
                            continue 'outer;
                        };

                        def.name = lit.value();
                    } else if meta.path.is_ident("route") {
                        let Some(lit) = type_util::expr_into_lit_str(meta.value) else {
                            emit_error!(meta.path, "'route' must be string literal");
                            continue 'outer;
                        };

                        def.routes.push(lit.value());
                    } else {
                        break 'value Meta::NameValue(meta);
                    }

                    continue 'outer;
                }

                // Intentionally leaving this as-is matchings for future use
                attr @ Meta::Path(_) => attr,
                attr @ Meta::List(_) => attr,
            };

            attrs.extend(attr.into_token_stream());
        }

        if def.name.is_empty() {
            // Fallback to method identifier
            def.name = def.method_ident.to_string();
        }

        let serializer_method;

        let method_ident = &def.method_ident;
        let method_name = &def.name;

        let tok_input = {
            let args = item
                .sig
                .inputs
                .into_iter()
                .map(|arg| {
                    if let syn::FnArg::Typed(arg) = arg {
                        arg
                    } else {
                        abort!(arg, "Can't set receiver token here")
                    }
                })
                .collect::<Vec<_>>();

            let mut types_ser = Vec::with_capacity(args.len());
            let mut types_de = Vec::with_capacity(args.len());

            for arg in &args {
                let Some((ser, de)) = type_util::retr_ser_de_params(&arg.ty) else {
                    // Do nothing; this is error case.
                    emit_error!(arg, "Serde parameter retrieval failed");
                    return;
                };

                types_ser.push(ser);
                types_de.push(de);
            }

            let type_idents = args
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    if let syn::Pat::Ident(ident) = &*arg.pat {
                        ident.ident.clone()
                    } else {
                        syn::Ident::new(&format!("___{index}"), arg.pat.span())
                    }
                })
                .collect::<Vec<_>>();

            assert!(type_idents.len() == types_ser.len());

            serializer_method = {
                let tok_return = if type_idents.is_empty() {
                    quote!(())
                } else {
                    quote!((#(#type_idents),*))
                };

                let tok_input = if type_idents.is_empty() {
                    quote!()
                } else {
                    let zipped_tokens = type_idents
                        .iter()
                        .zip(types_ser.iter())
                        .map(|(ident, ty)| quote!(#ident: #ty));
                    quote!(#(#zipped_tokens),*)
                };

                quote_spanned!(
                    item_span =>
                    #vis_outer fn #method_ident<'___ser>(#tok_input) -> (
                        #method_ident::Fn,
                        <#method_ident::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>
                    ) {
                        (
                            #method_ident::Fn,
                            #tok_return
                        )
                    }
                )
            };

            let gen_simple_de_type = no_recv || self.no_recv || self.no_param_recv_newtype;
            let (de_recv_type, tok_de_type) = if !gen_simple_de_type {
                // Creates new type for each deserialization types.
                //
                // This is to:
                // - Specify `#[serde(borrow)]` for each types
                //   - Which tries to borrow from orginal buffer when it's possible
                // - Imrpove ergonomics when dealing with resulting deserialzation types
                //   - If the function have multiple parameters, you can directly dereference the
                //     resulting type to get the inner type. (a newtype wrapper which implements
                //     `Deref`)
                //   - If multiple parameters are specified, the resulting type will be struct which
                //     contains all parameters as its field names.

                if types_de.is_empty() {
                    (quote!(()), quote!())
                } else if types_de.len() == 1 {
                    let ty = types_de.first().unwrap();
                    let has_lifetime = type_util::has_any_lifetime(ty);
                    let lifetime = has_lifetime.then(|| quote!(<'___de>));
                    let borrow = has_lifetime.then(|| quote!(#[serde(borrow)]));

                    (
                        quote!(ParamRecv #lifetime),
                        quote_spanned!(item_span =>
                            #[derive(serde::Deserialize)]
                            #vis_inner struct ParamRecv #lifetime ( #borrow #ty );

                            impl #lifetime ::std::ops::Deref for ParamRecv #lifetime {
                                type Target = #ty;

                                fn deref(&self) -> &Self::Target {
                                    &self.0
                                }
                            }
                        ),
                    )
                } else {
                    let mut has_any_lifetime = false;

                    let serde_types_de = types_de
                        .iter()
                        .map(|x| {
                            let borrow = type_util::has_any_lifetime(x);
                            has_any_lifetime |= borrow;

                            if borrow {
                                quote!(#[serde(borrow)])
                            } else {
                                quote!()
                            }
                        })
                        .collect::<Vec<_>>();

                    let lifetime = if has_any_lifetime {
                        quote!(<'___de>)
                    } else {
                        quote!()
                    };

                    (
                        quote_spanned!(item_span => self::ParamRecv #lifetime),
                        quote_spanned!(item_span =>
                            #[derive(serde::Deserialize)]
                            #vis_inner struct ParamRecv #lifetime {
                                #(
                                    #serde_types_de
                                    #vis_inner #type_idents: #types_de
                                ),*
                            }
                        ),
                    )
                }
            } else {
                (
                    quote!(ParamRecv<'___de>),
                    quote!(type ParamRecv<'___de> = (#(#types_de), *);),
                )
            };

            quote_spanned!(item_span =>
                impl ___macros::NotifyMethod for Fn {
                    type ParamSend<'___ser> = (#(#types_ser), *);
                    type ParamRecv<'___de> = #de_recv_type;

                    const METHOD_NAME: &'static str = #method_name;
                }

                #tok_de_type
            )
        };

        let tok_output = if let syn::ReturnType::Type(_, ty) = item.sig.output {
            def.is_req = true;

            // Check if it defines result type
            let opt_ok_err = 'find_result_t: {
                let Type::Path(syn::TypePath {
                    path:
                        syn::Path {
                            leading_colon: None,
                            segments,
                            ..
                        },
                    ..
                }) = &*ty
                else {
                    break 'find_result_t None;
                };

                let last = segments.last().unwrap();
                if last.ident != "Result" {
                    break 'find_result_t None;
                }

                let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
                    break 'find_result_t None;
                };

                if args.args.len() != 2 {
                    break 'find_result_t None;
                }

                let (GenericArgument::Type(ok), GenericArgument::Type(err)) =
                    (&args.args[0], &args.args[1])
                else {
                    break 'find_result_t None;
                };

                Some((ok, err))
            };

            if let Some((ok, err)) = opt_ok_err {
                let (Some((ok_ser, ok_de)), Some((err_ser, err_de))) = (
                    type_util::retr_ser_de_params(ok),
                    type_util::retr_ser_de_params(err),
                ) else {
                    emit_error!(
                        ty,
                        "Failed to retrieve serialization/deserialization types for result type"
                    );
                    return;
                };

                quote!(
                    impl ___macros::RequestMethod for Fn {
                        type OkSend<'___ser> = #ok_ser;
                        type OkRecv<'___de> = #ok_de;
                        type ErrSend<'___ser> = #err_ser;
                        type ErrRecv<'___de> = #err_de;
                    }
                )
            } else {
                let Some((ok_ser, ok_de)) = type_util::retr_ser_de_params(&ty) else {
                    emit_error!(
                        ty,
                        "Failed to retrieve serialization/deserialization types for return type"
                    );
                    return;
                };

                quote!(
                    impl ___macros::RequestMethod for Fn {
                        type OkSend<'___ser> = #ok_ser;
                        type OkRecv<'___de> = #ok_de;
                        type ErrSend<'___ser> = ();
                        type ErrRecv<'___de> = ();
                    }
                )
            }
        } else {
            Default::default()
        };

        out.extend(quote!(
            #vis_outer mod #method_ident {
                use super::*; // Make transparent to parent module

                #vis_inner struct Fn;

                #tok_input

                #tok_output
            }

            // #vis_outer fn #method_ident(_: #method_ident::Fn) {}
            #attrs
            #serializer_method
        ));

        if !no_recv {
            // Only generate receiver part if it's not explicitly disabled
            self.handled_methods.push(def);
        }
    }
}
