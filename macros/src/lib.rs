//! # Procedural Macro for Generating RPC Signatures
//!
//! This documentation explains the procedural macro used for generating RPC signatures, focusing on
//! key concepts and attributes.
//!
//! ## Key Concepts
//!
//! - **Method Name**: The identifier for a method. Senders use this name to serialize requests,
//!   while receivers use it to locate the appropriate handler through an 'exact match' strategy.
//!   This is the default approach for filtering inbound requests and notifications.
//!
//! - **Method Route**: In server-side method routing, you can specify additional routing
//!   configurations for each method. This is useful for creating methods that respond to multiple
//!   names or patterns (if supported by the router). Method routes are not applicable to outbound
//!   requests or notifications.
//!
//! - **Request Method**: A method that returns a value. The sender waits for a response from this
//!   type of method.
//!
//! - **Notification Method**: A method that does not return a value. It operates on a
//!   'fire-and-forget' basis, meaning the sender has no confirmation of the receiver having
//!   received the request.
//!
//! ## Attributes
//!
//! ### Service Attribute: `#[rpc_it::service(<ATTRS>)]`
//!
//! - `flatten`
//!     - When specified, the generated code is integrated into the current module.
//! - `name_prefix = "<PREFIX>"`
//!     - Appends a specified prefix to every method name.
//! - `route_prefix = "<PREFIX>"`
//!     - Appends a specified prefix to every method route.
//! - `rename_all = "<CASE>"`
//!     - Renames all default method names according to the specified case convention.
//!         - Explicit renamings using `#[name = "<NAME>"]` are exempt from this rule.
//!     - Supported case conventions: `snake_case`, `camelCase`, `PascalCase`,
//!       `SCREAMING_SNAKE_CASE`, `kebab-case`. The convention follows rules similar to
//!       `serde(rename_all = "<CASE>")`.
//! - `vis= "<VIS>"`
//!     - Sets the visibility of generated methods. Defaults to private if unspecified.
//!
//! ### Method Attributes
//!
//! - `[name = "<NAME>"]`
//!     - Renames the method to the specified string.
//! - `[route = "<ROUTE>"]`
//!     - Adds an additional route to the method.
//!
//! ## Serialization / Deserialization Rules
//!
//! - Single-argument methods: The argument is serialized as-is.
//! - Multi-argument methods: Arguments are serialized as a tuple (array in most serialization
//!   formats).
//!     - To serialize a single argument as a tuple, wrap it in an additional tuple (e.g., `T`
//!       becomes `(T,)`).
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
//!   fn method_notify(arg: (i32, i32));
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
//! ## Client side
//!
//! ```ignore
//!
//! let client: RequestSender = unimplemented!();
//!
//! client.try_call(module_name::method_req, )
//! ```
//!

use std::mem::take;

use convert_case::Case;
use proc_macro_error::{abort, emit_error, proc_macro_error};

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    parse_quote_spanned, punctuated::Punctuated, spanned::Spanned, ForeignItem, GenericArgument,
    Meta, PathSegment, Token, Type,
};
use tap::Pipe;

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
    methods: Vec<MethodDef>,
}

struct MethodDef {
    is_req: bool,
    method_ident: syn::Ident,
    name: String,
    routes: Vec<String>,
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
            out.extend(quote!({
                use super::*; // Make transparent to parent module
                #out_body
            }))
        }
    }

    fn generate_item_fn(
        &mut self,
        item: syn::ForeignItemFn,
        vis_offset: usize,
        out: &mut TokenStream,
    ) {
        /*
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

        // 1. Analyze visibility and name

        // 2. TODO: Generate method router (what kind of data required?)

        // 3. Analyze input types; generate `impl NotifyMethod`

        // 4. Analyze output types; generate `impl RequestMethod`

        let vis_outer = elevate_vis_level(item.vis, vis_offset);
        let vis_inner = elevate_vis_level(vis_outer.clone(), 1);
        let mut docs = TokenStream::new();

        let mut def = MethodDef {
            is_req: false,
            name: Default::default(),
            method_ident: item.sig.ident,
            routes: Vec::new(),
        };

        if let Some(case) = &self.rename_all {
            def.name = convert_case::Casing::to_case(&def.name, *case);
        }

        for attr in item.attrs {
            match attr.meta {
                Meta::Path(_) => (),
                Meta::List(_) => (),
                Meta::NameValue(ref meta) if meta.path.is_ident("doc") => {
                    docs.extend(attr.into_token_stream());
                }
                Meta::NameValue(meta) => {
                    if meta.path.is_ident("name") {
                        if !def.name.is_empty() {
                            emit_error!(meta, "Duplicated name attribute");
                            continue;
                        }

                        let Some(lit) = expr_into_lit_str(meta.value) else {
                            emit_error!(meta.path, "'name' must be string literal");
                            continue;
                        };

                        def.name = lit.value();
                    } else if meta.path.is_ident("route") {
                        let Some(lit) = expr_into_lit_str(meta.value) else {
                            emit_error!(meta.path, "'route' must be string literal");
                            continue;
                        };

                        def.routes.push(lit.value());
                    }
                }
            }
        }

        if def.name.is_empty() {
            // Fallback to method identifier
            def.name = def.method_ident.to_string();
        }

        let serializer_method;

        let method_ident = &def.method_ident;
        let method_name = &def.name;

        let tok_input = {
            // TODO: Generate Input Type impl Trait
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
                let Some((ser, de)) = retr_ser_de_params(&arg.ty) else {
                    // Do nothing; this is error case.
                    emit_error!(arg, "Serde parameter retrieval failed");
                    return;
                };

                types_ser.push(ser);
                types_de.push(de);
            }

            let make_into_tuple = |x: &[Type]| {
                if x.len() == 1 {
                    let x = &x[0];
                    quote!(#x)
                } else {
                    quote!((#(#x),*))
                }
            };

            let types_ser_tup = make_into_tuple(&types_ser);
            let types_de_tup = make_into_tuple(&types_de);

            serializer_method = {
                let idents = args
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

                assert!(idents.len() == types_ser.len());

                let tok_return = if idents.is_empty() {
                    quote!(())
                } else {
                    quote!((#(#idents),*))
                };

                let tok_input = if idents.is_empty() {
                    quote!()
                } else {
                    let zipped_tokens = idents
                        .iter()
                        .zip(types_ser.iter())
                        .map(|(ident, ty)| quote!(#ident: #ty));
                    quote!(#(#zipped_tokens),*)
                };

                quote!(
                    #vis_outer fn #method_ident<'___ser>(#tok_input) -> (
                        #method_ident::Fn,
                        <#method_ident::Fn as ::rpc_it::macros::NotifyMethod>::ParamSend<'___ser>
                    ) {
                        (
                            #method_ident::Fn,
                            #tok_return
                        )
                    }
                )
            };

            quote!(
                impl ___crate::NotifyMethod for Fn {
                    type ParamSend<'___ser> = #types_ser_tup;
                    type ParamRecv<'___de> = #types_de_tup;

                    const METHOD_NAME: &'static str = #method_name;
                }
            )
        };

        let tok_output = if let syn::ReturnType::Type(_, ty) = item.sig.output {
            def.is_req = true;

            // TODO: Generate Output Type impl Trait

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
                let (Some((ok_ser, ok_de)), Some((err_ser, err_de))) =
                    (retr_ser_de_params(ok), retr_ser_de_params(err))
                else {
                    emit_error!(
                        ty,
                        "Failed to retrieve serialization/deserialization types for result type"
                    );
                    return;
                };

                quote!(
                    impl ___crate::RequestMethod for Fn {
                        type OkSend<'___ser> = #ok_ser;
                        type OkRecv<'___de> = #ok_de;
                        type ErrSend<'___ser> = #err_ser;
                        type ErrRecv<'___de> = #err_de;
                    }
                )
            } else {
                let Some((ok_ser, ok_de)) = retr_ser_de_params(&ty) else {
                    emit_error!(
                        ty,
                        "Failed to retrieve serialization/deserialization types for return type"
                    );
                    return;
                };

                quote!(
                    impl ___crate::RequestMethod for Fn {
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
                use ::rpc_it::macros as ___crate;
                use super::*; // Make transparent to parent module

                #vis_inner struct Fn;

                #tok_input

                #tok_output
            }

            // #vis_outer fn #method_ident(_: #method_ident::Fn) {}
            #docs
            #serializer_method
        ));

        self.methods.push(def);
    }
}

fn retr_ser_de_params(ty: &Type) -> Option<(Type, Type)> {
    // TODO:
    // - Lifetime parameter handling; for non-static lifetimes.
    // - '__' type handling

    // 1. Elide lifetime for serialization
    // 2. Append reference for serialization

    // ---

    // Detect if type starts with `__<>`, which means separate serialization/deserialization types.
    let split_ser_de_type = ty
        .pipe(|x| {
            if let Type::Path(syn::TypePath { path, .. }) = x {
                Some(path)
            } else {
                None
            }
        })
        .and_then(|x| x.segments.first().filter(|_| x.segments.len() == 1))
        .filter(|x| x.ident == "__")
        .and_then(|x| {
            if let syn::PathArguments::AngleBracketed(generics) = &x.arguments {
                if generics.args.len() != 2 {
                    emit_error!(generics, "For '__' generic type ... Expected 2 arguments");
                    return None;
                }

                fn retr_type(x: &GenericArgument) -> Option<&Type> {
                    if let GenericArgument::Type(ty) = x {
                        Some(ty)
                    } else {
                        emit_error!(x, "Non-type generic is not allowed");
                        None
                    }
                }

                Some((retr_type(&generics.args[0])?, retr_type(&generics.args[1])?))
            } else {
                None
            }
        });

    let (ty_ser, ty_de) = split_ser_de_type.unwrap_or((ty, ty));
    let [mut ty_ser, mut ty_de] = [ty_ser, ty_de].map(Clone::clone);

    let life_ser: syn::Lifetime = parse_quote_spanned! { ty_ser.span() => '___ser };
    let life_de: syn::Lifetime = parse_quote_spanned! { ty_de.span() => '___de };

    if let syn::Type::Reference(ty) = &mut ty_ser {
        ty.lifetime = Some(life_ser.clone());
    } else {
        ty_ser = Type::Reference(syn::TypeReference {
            and_token: Token![&](ty_ser.span()),
            lifetime: Some(life_ser.clone()),
            mutability: None,
            elem: Box::new(ty_ser),
        })
    }

    replace_lifetime_occurence(&mut ty_ser, &life_ser, true);
    replace_lifetime_occurence(&mut ty_de, &life_de, false);

    Some((ty_ser, ty_de))
}

// Replace 'EVERY' lifetime occurrences into given lifetime.
fn replace_lifetime_occurence(a: &mut Type, life: &syn::Lifetime, skip_static: bool) {
    fn replace_inner(a: &mut syn::Lifetime, life: &syn::Lifetime, skip_static: bool) {
        if skip_static && a.ident == "static" {
            return;
        }

        a.ident = life.ident.clone();
    }

    match a {
        Type::Array(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::BareFn(_) => emit_error!(a, "You can't use function type here"),
        Type::Group(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::ImplTrait(_) => emit_error!(a, "You can't use impl trait here"),
        Type::Infer(_) => emit_error!(a, "You can't use infer type here"),
        Type::Macro(_) => emit_error!(a, "You can't use macro type here"),
        Type::Never(_) => emit_error!(a, "You can't use never type here"),
        Type::Paren(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::Ptr(x) => emit_error!(x, "You can't use pointer type here"),
        Type::Slice(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::Verbatim(_) => emit_error!(a, "Failed to parse type"),

        Type::Tuple(tup) => tup
            .elems
            .iter_mut()
            .for_each(|x| replace_lifetime_occurence(x, life, skip_static)),

        Type::TraitObject(x) => x.bounds.iter_mut().for_each(|x| match x {
            syn::TypeParamBound::Trait(tr) => {
                if let Some(lf) = &mut tr.lifetimes {
                    lf.lifetimes.iter_mut().for_each(|x| {
                        if let syn::GenericParam::Lifetime(lf) = x {
                            replace_inner(&mut lf.lifetime, life, skip_static)
                        }
                    })
                }
            }
            syn::TypeParamBound::Lifetime(x) => replace_inner(x, life, skip_static),
            syn::TypeParamBound::Verbatim(_) => emit_error!(x, "Failed to parse type"),
            _ => (),
        }),

        Type::Reference(x) => {
            if let Some(lf) = &mut x.lifetime {
                replace_inner(lf, life, skip_static)
            }

            replace_lifetime_occurence(&mut x.elem, life, skip_static)
        }

        Type::Path(pat) => {
            if let Some(qs) = &mut pat.qself {
                replace_lifetime_occurence(&mut qs.ty, life, skip_static);
            }

            pat.path.segments.iter_mut().for_each(
                |syn::PathSegment {
                     ident: _,
                     arguments,
                 }| match arguments {
                    syn::PathArguments::None => (),
                    syn::PathArguments::AngleBracketed(items) => {
                        items.args.iter_mut().for_each(|x| {
                            if let GenericArgument::Lifetime(lf) = x {
                                replace_inner(lf, life, skip_static)
                            }
                        });
                    }
                    syn::PathArguments::Parenthesized(items) => {
                        items
                            .inputs
                            .iter_mut()
                            .for_each(|x| replace_lifetime_occurence(x, life, skip_static));

                        if let syn::ReturnType::Type(_, ty) = &mut items.output {
                            replace_lifetime_occurence(ty, life, skip_static);
                        }
                    }
                },
            );
        }

        _ => (),
    }
}

fn elevate_vis_level(mut in_vis: syn::Visibility, amount: usize) -> syn::Visibility {
    // - pub(super) -> pub(super::super)
    // - None -> pub(super)
    // - pub(in crate::...) -> absolute; as is
    // - pub(in super::...) -> pub(in super::super::...)

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
