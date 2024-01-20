use super::DataModel;
use super::MethodDef;
use crate::type_util;
use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use proc_macro_error::emit_error;
use proc_macro_error::emit_warning;
use quote::{quote, quote_spanned};
use std::collections::BTreeMap;
use std::mem::take;
use syn::{spanned::Spanned, Meta};

impl DataModel {
    pub(crate) fn generate_route(
        &mut self,
        item: syn::ItemConst,
        vis_offset: usize,
        out: &mut TokenStream,
    ) {
        pub(crate) struct GenDesc {
            variant_ident: Option<syn::Ident>,
            no_default_route: bool,
            attrs: Vec<syn::Attribute>,
            def: MethodDef,
        }

        impl GenDesc {
            pub(crate) fn new(def: MethodDef) -> Self {
                Self {
                    variant_ident: None,
                    no_default_route: false,
                    attrs: Default::default(),
                    def,
                }
            }

            pub(crate) fn setup_by_struct_expr(
                &mut self,
                expr: &syn::ExprStruct,
                no_default_route: bool,
            ) {
                self.variant_ident = expr.path.get_ident().cloned();
                self.no_default_route = no_default_route;

                for field in &expr.fields {
                    let syn::Member::Named(mem) = &field.member else {
                        emit_error!(expr, "failed to parse expression");
                        continue;
                    };

                    if mem == "no_default_route" {
                        self.no_default_route = true;
                    } else if mem == "routes" {
                        let syn::Expr::Array(syn::ExprArray { elems, .. }) = &field.expr else {
                            emit_error!(expr, "failed to parse expression");
                            continue;
                        };

                        for elem in elems {
                            let Some(lit) = type_util::expr_into_lit_str(elem.clone()) else {
                                emit_error!(expr, "failed to parse expression");
                                continue;
                            };

                            self.def.routes.push(lit);
                        }
                    } else {
                        emit_error!(field, "Unknown field");
                    }
                }

                if self.no_default_route {
                    self.def.routes.clear();
                }
            }
        }

        // ==== Logic Begin ====

        let mut generate_targets = Vec::<GenDesc>::new();
        let item_span = item.ident.span();

        // Parse const item attribute to get route name
        let mut all_attrs = item.attrs;
        let mut global_no_default_route = false;
        let mut generate_install = false;
        let mut generate_direct = false;

        all_attrs.retain(|x| {
            match &x.meta {
                Meta::Path(p) => {
                    if p.is_ident("no_default_route") {
                        global_no_default_route = true;
                    } else if p.is_ident("install") {
                        generate_install = true;
                    } else if p.is_ident("direct") {
                        generate_direct = true;
                    } else {
                        return true;
                    }
                }
                _ => return true,
            };

            false
        });

        // ==== Method Definition Retrieval ====

        // If expr is `= ALL` then generate all methods. Otherwise, generate only specified methods.
        // This shoud be `[<<method_name>>, <<alias>> = <<method_name>>, ...]`
        match *item.expr {
            syn::Expr::Path(syn::ExprPath {
                path, qself: None, ..
            }) if path.is_ident("ALL") => {
                if global_no_default_route && (generate_install || generate_direct) {
                    emit_warning!(item_span, "ALL is specified, but no_default_route is set");
                }

                generate_targets.extend(self.handled_methods.iter().cloned().map(GenDesc::new));
            }

            syn::Expr::Path(syn::ExprPath {
                path, qself: None, ..
            }) if path.is_ident("ALL_PASCAL_CASE") => {
                if global_no_default_route && (generate_install || generate_direct) {
                    emit_warning!(
                        item_span,
                        "ALL_PASCAL_CASE is specified, but no_default_route is set"
                    );
                }

                generate_targets.extend(self.handled_methods.iter().map(|x| GenDesc {
                    variant_ident: {
                        let str = x.method_ident.to_string().to_case(Case::Pascal);
                        Some(syn::Ident::new(&str, x.method_ident.span()))
                    },
                    attrs: Default::default(),
                    no_default_route: false,
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
                            qself: None,
                            path,
                            attrs,
                        }) => {
                            if let Some(def) = self.find_method_by_path(&path) {
                                let mut new_desc = GenDesc::new(def.clone());
                                new_desc.attrs = attrs;
                                generate_targets.push(new_desc);
                            } else {
                                emit_error!(path, "Unknown method");
                            }
                        }

                        syn::Expr::Assign(syn::ExprAssign { left, right, .. }) => {
                            let syn::Expr::Path(expr_path) = *left else {
                                emit_error!(left, "Expected path");
                                continue;
                            };

                            if let Some(def) = self.find_method_by_path(&expr_path.path) {
                                let mut new_desc = GenDesc::new(def.clone());
                                new_desc.attrs = expr_path.attrs;
                                generate_targets.push(new_desc);
                            } else {
                                emit_error!(expr_path, "Unknown method");
                                continue;
                            }

                            let desc = generate_targets.last_mut().unwrap();
                            match &*right {
                                syn::Expr::Struct(expr_struct) => {
                                    desc.setup_by_struct_expr(expr_struct, global_no_default_route);
                                }
                                syn::Expr::Path(path) => {
                                    desc.variant_ident =
                                        Some(path.path.get_ident().unwrap().clone());
                                }
                                _ => {
                                    emit_error!(
                                        right,
                                        "Expected `MethodAlias` or `MethodAlias {..}`"
                                    )
                                }
                            }
                        }

                        syn::Expr::Struct(mut strt) => {
                            if let Some(def) = self.find_method_by_path(&strt.path) {
                                let mut new_desc = GenDesc::new(def.clone());
                                new_desc.attrs = take(&mut strt.attrs);
                                generate_targets.push(new_desc);
                            } else {
                                emit_error!(strt, "Unknown method");
                                continue;
                            }

                            let desc = generate_targets.last_mut().unwrap();
                            desc.setup_by_struct_expr(&strt, global_no_default_route);
                        }

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

        // ==== Route Definition Generate ====

        let vis_this = type_util::elevate_vis_level(item.vis.clone(), vis_offset);
        let ident_this = &item.ident;

        let tok_enum_title = { quote!(#vis_this enum #ident_this) };

        let tok_enum_variants = if generate_targets.is_empty() {
            quote!(__EmptyVariant(::std::marker::PhantomData<R>))
        } else {
            let tokens = generate_targets.iter().map(
                |GenDesc {
                     variant_ident,
                     no_default_route: _,
                     attrs,
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
                        #(#attrs)*
                        #ident(___crate::cached:: #type_path <R, self:: #method_ident :: Fn>)
                    )
                },
            );

            quote!(#(#tokens),*)
        };

        // ========================================================== Install Contents ===|

        let handler_impl_clone = (generate_targets.len() > 1).then(|| quote!(+ Clone));
        let tok_install_contents = generate_targets.iter().enumerate().map(
            |(
                index,
                GenDesc {
                    variant_ident,
                    no_default_route,
                    attrs: _,
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

                let no_default_route = *no_default_route || global_no_default_route;
                let name = syn::LitStr::new(name.as_str(), name.span());
                let route_strs = (!no_default_route)
                    .then_some(&name)
                    .into_iter()
                    .chain(routes.iter())
                    .map(|x| quote!(#x));

                let is_last = index == generate_targets.len() - 1;
                let clone_handler = (!is_last).then(|| quote!(.clone()));
                let method_name_str = method_ident.to_string();

                let warn_req_on_noti = (!is_req).then(|| {
                    quote!(if ___ib.is_request() {
                        *___inbound = Some(___ib);
                        return Err(___route::ExecError::RequestOnNotifyHandler);
                    })
                });

                quote!({
                    let ___handler = ___handler #clone_handler;

                    ___router.push_handler(move |___inbound| {
                        let ___ib = ___inbound.take().unwrap();

                        #warn_req_on_noti

                        ___handler(
                            #ident_this :: #ident (
                                ___crate::cached:: #type_path ::__internal_create(
                                    ___ib,
                                ).map_err(|(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                })?
                            )
                        );

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

        let tok_func_install = generate_install.then(|| {
            quote_spanned!(item_span =>
                #vis_this fn install<B, E>(
                    ___router: &mut ___route::RouterBuilder<R, B>,
                    ___handler: impl Fn(Self) + Send + Sync + 'static #handler_impl_clone,
                    mut ___on_route_error: impl FnMut(&str, &str, B::Error) -> E,
                ) where
                    B: ___route::RouterFuncBuilder,
                    E: Into<___route::RouteFailResponse>,
                {
                    use ___route::RouteFailResponse as ___RF;

                    #(#tok_install_contents)*
                }
            )
        });

        // ========================================================== Direct Contents ===|

        let tok_direct_contents = generate_targets.iter().enumerate().map(
            |(
                index,
                GenDesc {
                    variant_ident,
                    no_default_route,
                    attrs: _,
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
                    quote!(___RQ)
                } else {
                    quote!(___NT)
                };

                let no_default_route = *no_default_route || global_no_default_route;
                let name = syn::LitStr::new(name.as_str(), name.span());
                let type_name = quote!(
                    #type_path :: <R, self:: #method_ident :: Fn>
                );
                let route_strs = (!no_default_route)
                    .then_some(&name)
                    .into_iter()
                    .chain(routes.iter())
                    .map(|x| quote!(#x));

                (
                    quote!(
                        #(#route_strs)|* => #index,
                    ),
                    quote!(
                        #index => Self::#ident(
                            #type_name :: __internal_create(___ib)
                                .map_err(|(___ib, ___err)| {
                                    (___ib, ___err.into())
                                })?
                        )
                    ),
                )
            },
        );

        let tok_func_direct = generate_direct.then(|| {
            let (iter_match_index_fn, iter_route_fn): (Vec<_>, Vec<_>) =
                tok_direct_contents.unzip();

            quote!(
                fn ___inner_match_index(___route: &str) -> Option<usize> {
                    Some(match ___route {
                        #(#iter_match_index_fn)*
                        _ => return None
                    })
                }

                #vis_this fn matches(___route: &str) -> bool {
                    Self::___inner_match_index(___route).is_some()
                }

                #vis_this fn route(___ib: ___crate::Inbound<R>)
                    -> Result<Self, (___crate::Inbound<R>, ___route::ExecError)>
                {
                    use ___crate::cached::Request as ___RQ;
                    use ___crate::cached::Notify as ___NT;

                    let Some(___index) = Self::___inner_match_index(___ib.method()) else {
                        return Err((___ib, ___route::ExecError::RouteFailed));
                    };

                    let ___item = match ___index {
                        #(#iter_route_fn,)*
                        _ => unreachable!(),
                    };

                    Ok(___item)
                }
            )
        });

        let tok_direct_try_from = generate_direct.then(|| {
            quote!(
                impl<R: ___crate::Config> ::std::convert::TryFrom<___crate::Inbound<R>>
                    for #ident_this<R>
                {
                    type Error = (___crate::Inbound<R>, ___route::ExecError);

                    fn try_from(___ib: ___crate::Inbound<R>)
                        -> Result<Self, Self::Error>
                    {
                        Self::route(___ib)
                    }
                }
            )
        });

        // ==== Direct Routing Validity Check ====

        generate_targets.iter().fold(
            BTreeMap::new(),
            |mut direct_routings,
             GenDesc {
                 def:
                     MethodDef {
                         method_ident,
                         routes,
                         name,
                         ..
                     },
                 no_default_route,
                 ..
             }| {
                let no_default_route = *no_default_route || global_no_default_route;
                let name = syn::LitStr::new(name.as_str(), method_ident.span());
                let route_strs = (!no_default_route)
                    .then_some(&name)
                    .into_iter()
                    .chain(routes.iter());

                for route in route_strs {
                    if let Some((prev_route, prev_method)) =
                        direct_routings.insert(route.value(), (route.clone(), method_ident))
                    {
                        let route_str = route.value();
                        emit_error!(name, "Duplicated route '{}'", route_str);
                        emit_error!(
                            prev_route,
                            "Duplicated route between '{}' and '{}'",
                            method_ident,
                            prev_method,
                        );
                        emit_error!(
                            route,
                            "Duplicated route between '{}' and '{}'",
                            method_ident,
                            prev_method,
                        );
                        emit_error!(item_span, "Failed to generate routes");
                    }
                }

                direct_routings
            },
        );

        out.extend(quote_spanned!(item_span =>
            #(#all_attrs)*
            #tok_enum_title<R: ___crate::Config> {
                #tok_enum_variants
            }

            impl<R: ___crate::Config> #ident_this<R> {
                #tok_func_install
                #tok_func_direct
            }

            #tok_direct_try_from
        ))
    }
}
