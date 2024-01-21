use syn::{GenericArgument, Type};

use quote::{quote, quote_spanned, ToTokens};

use proc_macro_error::abort;

use proc_macro2::Span;

use proc_macro_error::emit_error;

use syn::spanned::Spanned;
use syn::Meta;

use std::mem::take;

use crate::type_util;

use super::MethodDef;

use proc_macro2::TokenStream;

use super::DataModel;

impl DataModel {
    pub(crate) fn generate_item_fn(
        &mut self,
        mut item: syn::ForeignItemFn,
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

        let vis_outer = type_util::elevate_vis_level(item.vis.clone(), vis_offset);
        let vis_inner = type_util::elevate_vis_level(vis_outer.clone(), 1);

        let mut attrs = TokenStream::new();
        let mut no_recv = false;

        let mut def = MethodDef {
            is_req: false,
            name: Default::default(),
            method_ident: item.sig.ident.clone(),
            routes: Vec::new(),
        };

        if let Some(case) = &self.rename_all {
            def.name = convert_case::Casing::to_case(&def.name, *case);
        }

        'outer: for mut attr in take(&mut item.attrs) {
            attr.meta = match attr.meta {
                Meta::Path(path) => 'ret: {
                    if path.is_ident("no_recv") {
                        no_recv = true;
                    } else {
                        break 'ret Meta::Path(path);
                    }

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

                        def.routes.push(lit);
                    } else {
                        break 'value Meta::NameValue(meta);
                    }

                    continue 'outer;
                }

                // Intentionally leaving this as-is matchings for future use
                Meta::List(meta) => 'ret: {
                    if false {
                    } else {
                        break 'ret Meta::List(meta);
                    }

                    continue 'outer;
                }
            };

            attrs.extend(attr.into_token_stream());
        }

        if def.name.is_empty() {
            // Fallback to method identifier
            def.name = def.method_ident.to_string();
        }

        self.fn_def_body(
            &mut def, item, item_span, &vis_outer, &vis_inner, no_recv, out, attrs,
        );

        if !no_recv {
            // Only generate receiver part if it's not explicitly disabled
            self.handled_methods.push(def);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn fn_def_body(
        &mut self,
        def: &mut MethodDef,
        item: syn::ForeignItemFn,
        item_span: Span,
        vis_outer: &syn::Visibility,
        vis_inner: &syn::Visibility,
        no_recv: bool,
        out: &mut TokenStream,
        attrs_fwd: TokenStream,
    ) {
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
                    #[cfg(any())]
                    {
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
                    }
                    {
                        let ty = types_de.first().unwrap();
                        (quote!(#ty), quote!())
                    }
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
            #attrs_fwd
            #serializer_method
        ));
    }
}
