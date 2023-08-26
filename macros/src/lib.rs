use std::collections::HashSet;

use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use proc_macro_error::{abort, emit_error, proc_macro_error};
use quote::quote;
use syn::{
    spanned::Spanned, FnArg, GenericArgument, Ident, Pat, ReturnType, Token, TraitItem,
    TraitItemFn, Type, VisRestricted, Visibility,
};

/// Defines new RPC service
///
/// All parameter types must implement both of [`serde::Serialize`] and [`serde::Deserialize`].
///
/// The trait name will be converted to snake_case module, and all related definitions will be
/// defined inside the module.
///
/// # Available Attributes For Methods
///
/// - `async_fn`: For requests(which has `->` return type), this passes the request object to the
///   handler, instead of waiting return value.
/// - `routes = "..."`: Additional routes for the method. This is useful when you want to have
///   multiple routes for the same method.
/// - `with_reuse`: Generate `*_with_reuse` series of methods. This is useful when you want to
///   optimize buffer allocation over multiple consecutive calls.
/// - `skip`: Do not generate any code from this. This is useful when you need just a trait method,
///   which can be used another default implementations.
///
#[proc_macro_error]
#[proc_macro_attribute]
pub fn service(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let tokens = proc_macro2::TokenStream::from(item);

    /*
        From comment example, this macro automatically implements:
            struct MyServiceLoader;
            impl MyServiceLoader {
                fn load(this: Arc<MyService>, service: &mut ServiceBuilder) {
                    service.register ...
                }
            }

        For client, this struct is defined ...
            struct MyServiceStub<'a>(Cow<'a, Transceiver>);
            impl<'a> MyServiceStub<'a> {
                pub fn new(trans: impl Into<Cow<'a, Transceiver>>) {

                }
            }
    */

    let Ok(ast) = syn::parse2::<syn::ItemTrait>(tokens) else {
        return proc_macro::TokenStream::new();
    };

    let module_name = format!("{}", ast.ident.to_string().to_case(Case::Snake));
    let module_name = Ident::new(&module_name, proc_macro2::Span::call_site());
    let original_vis = &ast.vis;
    let vis = match ast.vis.clone() {
        vis @ (syn::Visibility::Public(_) | syn::Visibility::Restricted(_)) => vis,
        syn::Visibility::Inherited => syn::Visibility::Restricted(VisRestricted {
            pub_token: Token![pub](proc_macro2::Span::call_site()),
            paren_token: Default::default(),
            in_token: None,
            path: Box::new(syn::parse2::<syn::Path>(quote!(super)).unwrap()),
        }),
    };

    let mut statefuls = Vec::new();
    let mut statelesses = Vec::new();
    let mut call_binds = Vec::new();
    let mut name_table = HashSet::new();

    let (functions, attrs): (Vec<_>, Vec<_>) = ast
        .items
        .iter()
        .filter_map(|x| if let TraitItem::Fn(x) = x { Some(x) } else { None })
        .map(|x| {
            let mut x = x.clone();
            let attrs = method_attrs(&mut x);
            (x, attrs)
        })
        .unzip();

    let non_functions = ast
        .items
        .iter()
        .filter_map(|x| if let TraitItem::Fn(_) = x { None } else { Some(x) })
        .collect::<Vec<_>>();

    for item in functions.iter().zip(&attrs) {
        if let Some(loaded) = generate_loader_item(item.0, item.1, &mut name_table) {
            match loaded {
                LoaderOutput::Stateful(stateful) => statefuls.push(stateful),
                LoaderOutput::Stateless(stateless) => statelesses.push(stateless),
            }
        }

        if let Some(caller) = generate_call_stubs(item.0, item.1, &vis) {
            call_binds.push(caller);
        }
    }

    let trait_signatures = generate_trait_signatures(&functions, &attrs);

    let output = quote!(
        #original_vis mod #module_name {
            #![allow(unused_parens)]
            #![allow(unused)]

            use super::*;

            use rpc_it::service as __sv;
            use rpc_it::service::macro_utils as __mc;
            use std::sync as __sc;
            use rpc_it::serde;

            #vis trait Service: Send + Sync + 'static + Clone {
                #trait_signatures
                #(#non_functions)*
            }

            #vis fn load_service_stateful_only<T: Service, R: __sv::Router>(
                __this: T,
                __service: &mut __sv::ServiceBuilder<R>
            ) -> __mc::RegisterResult {
                #(#statefuls;)*
                Ok(())
            }

            #vis fn load_service_stateless_only<T: Service>(
                __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>
            ) -> __mc::RegisterResult {
                #(#statelesses;)*
                Ok(())
            }

            #vis fn load_service<T:Service>(
                __this: T,
                __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>
            ) -> __mc::RegisterResult {
                load_service_stateful_only(__this, __service)?;
                load_service_stateless_only::<T>(__service)?;
                Ok(())
            }

            #[derive(Debug, Clone)]
            #vis struct Client(rpc_it::Sender);

            impl Client {
                #vis fn new(inner: rpc_it::Sender) -> Self {
                    Self(inner)
                }

                #vis fn into_inner(self) -> rpc_it::Sender {
                    self.0
                }

                #vis fn inner(&self) -> &rpc_it::Sender {
                    &self.0
                }

                #(#call_binds)*
            }
        }
    );

    output.into()
}

enum LoaderOutput {
    Stateful(TokenStream),
    Stateless(TokenStream),
}

fn generate_loader_item(
    method: &TraitItemFn,
    attrs: &MethodAttrs,
    used_route_table: &mut HashSet<String>,
) -> Option<LoaderOutput> {
    if attrs.skip {
        return None;
    }

    let mut is_self_ref = false;
    let mut is_stateless = false;

    if let Some(receiver) = method.sig.receiver() {
        if receiver.reference.is_some() && receiver.colon_token.is_none() {
            if receiver.mutability.is_some() {
                emit_error!(receiver, "Only `&self` or `self: Arc<Self>` is allowed");
                return None;
            }

            is_self_ref = true;
        } else if receiver.colon_token.is_some() && receiver.reference.is_none() {
            is_self_ref = matches!(&*receiver.ty, syn::Type::Reference(_));
        }
    } else {
        is_stateless = true;
    };

    // Additional routes
    let is_pseudo_async = attrs.async_fn;
    let mut routes = Vec::with_capacity(1 + attrs.routes.len());
    let ident = &method.sig.ident;
    routes.push(method.sig.ident.to_string());

    for route in &attrs.routes {
        routes.push(route.value());
    }

    // Pairs of (is_ref, req-type)
    let (is_ref, inputs): (Vec<_>, Vec<_>) = method
        .sig
        .inputs
        .iter()
        .skip(if is_self_ref { 1 } else { 0 })
        .map(|input| {
            let syn::FnArg::Typed(pat) = input else {
                abort!(input, "unexpected argument type");
            };

            if let Type::Reference(r) = &*pat.ty {
                let inner = &r.elem;
                (true, Type::Verbatim(quote!(std::borrow::Cow<#inner>)))
            } else {
                (false, (*pat.ty).clone())
            }
        })
        .unzip();

    let tup_inputs = quote!((#(#inputs),*));
    let route_paths = quote!(&[#(#routes),*]);
    let unpack = if inputs.len() == 1 {
        quote!(__req)
    } else {
        let vals = (0..inputs.len()).map(|x| syn::Index::from(x));
        let ident = is_ref.iter().map(|x| if *x { quote!(&) } else { quote!() });
        quote!(#( #ident __req.#vals ),*)
    };

    for r in routes {
        if !used_route_table.insert(r.clone()) {
            emit_error!(method, "duplicated route: {}", r);
        }
    }

    let output = OutputType::new(&method.sig.output);

    Some(if is_stateless {
        let strm = if output.is_notify() {
            quote!(
                __service.register_notify_handler(#route_paths, |__req: #tup_inputs| {
                    T::#ident(#unpack);
                    Ok(())
                })?
            )
        } else {
            let treq = output.typed_req();

            if !is_pseudo_async {
                let rval = output.handle_sync_retval_to_response(Ident::new(
                    "__result",
                    method.sig.output.span(),
                ));

                quote!(
                    __service.register_request_handler(#route_paths, |__req: #tup_inputs, __rep: #treq| {
                        let __result = T::#ident(#unpack);
                        #rval;
                        Ok(())
                    })?
                )
            } else {
                quote!(
                    __service.register_request_handler(#route_paths, |__req: #tup_inputs, __rep: #treq| {
                        T::#ident(#unpack, __rep);
                        Ok(())
                    })?
                )
            }
        };
        LoaderOutput::Stateless(strm)
    } else {
        let this_param = if is_self_ref { quote!(&__this_2) } else { quote!(__this_2.clone()) };
        let strm = if output.is_notify() {
            quote!(
                let __this_2 = __this.clone();
                __service.register_notify_handler(#route_paths, move |__req: #tup_inputs| {
                    T::#ident(#this_param, #unpack);
                    Ok(())
                })?
            )
        } else {
            let treq = output.typed_req();

            if !is_pseudo_async {
                let rval = output.handle_sync_retval_to_response(Ident::new(
                    "__result",
                    method.sig.output.span(),
                ));

                quote!(
                    let __this_2 = __this.clone();
                    __service.register_request_handler(#route_paths, move |__req: #tup_inputs, __rep: #treq| {
                        let __result = T::#ident(#this_param, #unpack);
                        #rval;
                        Ok(())
                    })?
                )
            } else {
                quote!(
                    let __this_2 = __this.clone();
                    __service.register_request_handler(#route_paths, move |__req: #tup_inputs, __rep: #treq| {
                        T::#ident(#this_param, #unpack, __rep);
                        Ok(())
                    })?
                )
            }
        };
        LoaderOutput::Stateful(strm)
    })
}

fn generate_call_stubs(
    method: &TraitItemFn,
    attrs: &MethodAttrs,
    vis: &Visibility,
) -> Option<TokenStream> {
    if attrs.skip {
        return None;
    }

    let has_receiver = method.sig.receiver().is_some();

    let inputs = method
        .sig
        .inputs
        .iter()
        .skip(if has_receiver { 1 } else { 0 })
        .map(|arg| {
            let FnArg::Typed(pat) = arg else { abort!(arg, "unexpected argument type") };
            if !matches!(*pat.pat, Pat::Ident(_)) {
                abort!(arg, "Function argument pattern must be named identifier.");
            }
            pat
        })
        .collect::<Vec<_>>();

    let input_ref_args = inputs.iter().map(|x| *x).cloned().map(|mut x| {
        x.ty = match *x.ty {
            ty @ Type::Reference(_) => ty.into(),
            other => Type::Reference(syn::TypeReference {
                and_token: Token![&](other.span()),
                lifetime: None,
                mutability: None,
                elem: other.into(),
            })
            .into(),
        };
        x
    });
    let input_ref_arg_tokens = quote!(#(#input_ref_args),*);

    let input_idents = inputs
        .iter()
        .map(|x| *x)
        .cloned()
        .map(|x| {
            let syn::Pat::Ident(syn::PatIdent { ident, .. }) = &*x.pat else { unreachable!() };
            ident.clone()
        })
        .collect::<Vec<_>>();

    let method_ident = &method.sig.ident;
    let method_str = method_ident.to_string();
    let output = OutputType::new(&method.sig.output);

    let new_ident_suffixed =
        |sfx: &str| syn::Ident::new(&format!("{0}_{1}", method_ident, sfx), method_ident.span());
    let new_ident_prefixed =
        |sfx: &str| syn::Ident::new(&format!("{1}_{0}", method_ident, sfx), method_ident.span());
    let method_ident_deferred = new_ident_suffixed("deferred");

    Some(if output.is_notify() {
        let method_ident_with_reuse = new_ident_suffixed("with_reuse");
        let method_ident_deferred_with_reuse = new_ident_suffixed("deferred_with_reuse");

        let reuse_version = attrs.with_reuse.then(|| quote!(
            #[doc(hidden)]
            #vis async fn #method_ident_with_reuse(&self, buffer: &mut rpc_it::rpc::WriteBuffer,  #input_ref_arg_tokens) -> Result<(), rpc_it::SendError> {
                self.0.notify_with_reuse(buffer, #method_str, &(#(#input_idents),*)).await
            }

            #[doc(hidden)]
            #vis async fn #method_ident_deferred_with_reuse(&self, buffer: &mut rpc_it::rpc::WriteBuffer,  #input_ref_arg_tokens) -> Result<(), rpc_it::SendError> {
                self.0.notify_deferred_with_reuse(buffer, #method_str, &(#(#input_idents),*))
            }
        ));

        quote!(
            #vis async fn #method_ident(&self, #input_ref_arg_tokens) -> Result<(), rpc_it::SendError> {
                self.0.notify(#method_str, &(#(#input_idents),*)).await
            }


            #vis async fn #method_ident_deferred(&self, #input_ref_arg_tokens) -> Result<(), rpc_it::SendError> {
                self.0.notify_deferred(#method_str, &(#(#input_idents),*))
            }

            #reuse_version
        )
    } else {
        let (ok_tok, err_tok) = match &output {
            OutputType::Response(ok, err) => (quote!(#ok), quote!(#err)),
            OutputType::ResponseNoErr(ok) => (quote!(#ok), quote!(())),
            OutputType::Notify => unreachable!(),
        };

        let method_ident_request = new_ident_prefixed("request");

        quote!(
            #vis async fn #method_ident(&self, #input_ref_arg_tokens)
                -> Result<#ok_tok, rpc_it::TypedCallError<#err_tok>>
            {
                self.0.call_with_err(#method_str, &(#(#input_idents),*)).await
            }

            #vis async fn #method_ident_request(&self, #input_ref_arg_tokens)
                -> Result<rpc_it::TypedResponse<#ok_tok, #err_tok>, rpc_it::SendError>
            {
                let resp = self.0.request(#method_str, &(#(#input_idents),*)).await?;
                Ok(rpc_it::TypedResponse::new(resp.to_owned()))
            }

            #vis async fn #method_ident_deferred(&self, #input_ref_arg_tokens)
                -> Result<rpc_it::TypedResponse<#ok_tok, #err_tok>, rpc_it::SendError>
            {
                let resp = self.0.request_deferred(#method_str, &(#(#input_idents),*))?;
                Ok(rpc_it::TypedResponse::new(resp.to_owned()))
            }
        )
    })
}

fn generate_trait_signatures(items: &[TraitItemFn], attrs: &[MethodAttrs]) -> TokenStream {
    let tokens = items.iter().zip(attrs).map(|(method, attrs)| {
        let mut method = method.clone();

        // XXX: When static async method is stabilized, add support for it.
        // - For now, to not corrupt the async keyword usage, we only allow declaring pseudo-async
        //   method through method attribute
        if !attrs.async_fn || attrs.skip {
            return TraitItem::Fn(method);
        }

        if method.default.is_some() {
            abort!(method, "You can't provide default implementation for async handler");
        }

        let outputs = OutputType::new(&method.sig.output);
        if outputs.is_notify() {
            abort!(method, "Only request handler can be async!");
        }

        method.sig.inputs.push(syn::FnArg::Typed(syn::PatType {
            attrs: Vec::new(),
            colon_token: Default::default(),
            pat: syn::Pat::Wild(syn::PatWild {
                attrs: Vec::new(),
                underscore_token: Token![_](method.sig.output.span()),
            })
            .into(),
            ty: outputs.typed_req().into(),
        }));

        method.sig.output = ReturnType::Default;
        TraitItem::Fn(method)
    });

    quote!(#(#tokens)*)
}

#[derive(Default)]
struct MethodAttrs {
    async_fn: bool,
    skip: bool,
    routes: Vec<syn::LitStr>,
    with_reuse: bool,
}

fn method_attrs(method: &mut TraitItemFn) -> MethodAttrs {
    let mut attrs = MethodAttrs::default();

    for attr in std::mem::take(&mut method.attrs) {
        match &attr.meta {
            syn::Meta::Path(path) => {
                if path.is_ident("async_fn") {
                    attrs.async_fn = true;
                } else if path.is_ident("skip") {
                    attrs.skip = true;
                } else if path.is_ident("with_reuse") {
                    attrs.with_reuse = true;
                } else {
                    emit_error!(attr, "unexpected attribute")
                }
            }

            syn::Meta::List(_) => {
                emit_error!(attr, "unexpected attribute")
            }

            syn::Meta::NameValue(kv) => {
                if kv.path.get_ident().is_some_and(|x| x == "routes") {
                    let syn::Expr::Lit(syn::ExprLit { lit, .. }) = &kv.value else {
                        emit_error!(attr, "unexpected attribute");
                        continue;
                    };
                    let syn::Lit::Str(route) = lit else {
                        emit_error!(lit, "unexpected non-string literal attribute");
                        continue;
                    };
                    attrs.routes.push(route.clone());
                } else {
                    emit_error!(attr, "unexpected attribute")
                }
            }
        }
    }

    attrs
}

enum OutputType {
    Notify,
    ResponseNoErr(Type),
    Response(GenericArgument, GenericArgument),
}

impl OutputType {
    fn new(val: &syn::ReturnType) -> Self {
        let syn::ReturnType::Type(_, ty) = val else { return Self::Notify };

        let fb = || Self::ResponseNoErr((**ty).clone());
        let Type::Path(tp) = &**ty else { return fb() };
        let Some(first_seg) = tp.path.segments.first() else { return fb() };

        if first_seg.ident != "Result" {
            return fb();
        }

        let syn::PathArguments::AngleBracketed(ang) = &first_seg.arguments else {
            return fb();
        };

        let mut type_iter = ang.args.iter();
        let [Some(ok), Some(err)] = std::array::from_fn(|_| type_iter.next()) else {
            return fb();
        };

        Self::Response(ok.clone(), err.clone())
    }

    fn is_notify(&self) -> bool {
        matches!(self, Self::Notify)
    }

    fn typed_req(&self) -> Type {
        match self {
            OutputType::Notify => unimplemented!(),

            OutputType::ResponseNoErr(x) => {
                syn::parse2(quote!(rpc_it::TypedRequest<#x, ()>)).unwrap()
            }

            OutputType::Response(r, e) => {
                syn::parse2(quote!(rpc_it::TypedRequest<#r, #e>)).unwrap()
            }
        }
    }

    fn handle_sync_retval_to_response(&self, ident: Ident) -> TokenStream {
        match self {
            OutputType::Notify => unimplemented!(),

            OutputType::ResponseNoErr(_) => {
                quote!(__rep.ok(&#ident)?;)
            }

            OutputType::Response(_, _) => {
                quote!(
                    match #ident {
                        Ok(x) => __rep.ok(&x)?,
                        Err(e) => __rep.err(&e)?,
                    }
                )
            }
        }
    }
}
