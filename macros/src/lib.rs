use std::{borrow::Cow, collections::HashSet};

use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use proc_macro_error::{abort, emit_error, proc_macro_error};
use quote::quote;
use syn::{
    spanned::Spanned, GenericArgument, Ident, ReturnType, Token, TraitItem, TraitItemFn, Type,
    VisRestricted,
};

/// Defines new RPC service
///
/// All parameter types must implement both of [`serde::Serialize`] and [`serde::Deserialize`].
///
/// The trait name will be converted to snake_case module, and all related definitions will be
/// defined inside the module.
/// ```
/// #[rpc_it_macros::service]
/// pub trait MyService {
///   fn add(&self, a: i32, b: i32) -> i32;
///   
///   #[route += "div-2"]
///   #[route += "div3/*"]
///   #[route += "div44"]
///   fn div(&self, a: i32, b: i32) -> Result<i32, String>;
///
///   /// Treated as notify (no return type)
///   fn print(&self) {
///     // Default implementation can be provided.
///   }
///
///   /// May define stateless methods.
///   fn print_stateless() {}
///  
///   /// Treated as request (return type is `()`).
///   fn print_confirm(&self) -> () {}
/// }
///
/// pub struct MyServiceImpl;
///
/// impl my_service::Service for MyServiceImpl {
///   fn add(&self, a: i32, b: i32) -> i32 { a + b }
///   fn div(&self, a: i32, b: i32) -> Result<i32, String> {
///     if b == 0 { Err("failed!".into()) }
///     else { Ok(a / b) }
///   }
/// }
///
/// let mut service = rpc_it::service::ServiceBuilder::<rpc_it::service::ExactMatchRouter>::default();
///
/// // Loads only stateless methods.
/// my_service::Loader::load_stateless(&mut service);
///
/// // Loads all methods.
/// my_service::Loader::new(MyServiceImpl).load(&mut service);
/// ```
///
#[proc_macro_error]
#[proc_macro_attribute]
pub fn service(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    println!("attr .. \"{}\"", attr.to_string());
    println!("item .. \"{}\"", item.to_string());

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
    let vis = match ast.vis.clone() {
        vis @ (syn::Visibility::Public(_) | syn::Visibility::Restricted(_)) => vis,
        syn::Visibility::Inherited => syn::Visibility::Restricted(VisRestricted {
            pub_token: Token![pub](proc_macro2::Span::call_site()),
            paren_token: Default::default(),
            in_token: None,
            path: Box::new(syn::parse2::<syn::Path>(quote!(self)).unwrap()),
        }),
    };

    let mut statefuls = Vec::new();
    let mut statelesses = Vec::new();
    let mut name_table = HashSet::new();

    for item in &ast.items {
        let Some(loaded) = generate_loader_item(item, &mut name_table) else { continue };

        match loaded {
            LoaderOutput::Stateful(stateful) => statefuls.push(stateful),
            LoaderOutput::Stateless(stateless) => statelesses.push(stateless),
        }
    }

    let signatures = ast.items.iter().filter_map(generate_call_stubs).collect::<Vec<_>>();
    let trait_signatures = generate_trait_signatures(&ast.items);

    let output = quote!(
        mod #module_name {
            #![allow(unused_parens)]

            use rpc_it::service as __sv;
            use rpc_it::service::macro_utils as __mc;
            use std::sync as __sc;
            use rpc_it::serde;

            #vis trait Service: Send + Sync + 'static {
                #trait_signatures
            }

            #vis struct Loader;
            impl Loader {
                #vis fn load_stateful<T:Service, R: __sv::Router>(
                    __this: __sc::Arc<T>,
                    __service: &mut __sv::ServiceBuilder<R>
                ) -> __mc::RegisterResult {
                    #(#statefuls;)*
                    Ok(())
                }

                #vis fn load_stateless<'__l_a, T:Service>(
                    __service: &'__l_a mut __sv::ServiceBuilder<__sv::ExactMatchRouter>
                ) -> __mc::RegisterResult {
                    #(#statelesses;)*
                    Ok(())
                }

                #vis fn load<T:Service>(
                    __this: T,
                    __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>
                ) -> __mc::RegisterResult {
                    Self::load_stateful(__sc::Arc::new(__this), __service)
                         .and_then(|_| Self::load_stateless::<T>(__service))
                }
            }

            #vis struct Stub<'a>(std::borrow::Cow<'a, rpc_it::Transceiver>);

            impl<'a> Stub<'a> {
                #(#signatures)*

                async fn __call<Req, Rep, Err>(&self, method:&str, req: &Req)
                    -> Result<Rep, rpc_it::TypedCallError<Err>>
                where
                    Req: serde::Serialize,
                    Rep: serde::de::DeserializeOwned,
                    Err: serde::de::DeserializeOwned,
                {
                    self.0.call_with_err(method, req).await
                }
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
    item: &TraitItem,
    used_route_table: &mut HashSet<String>,
) -> Option<LoaderOutput> {
    let TraitItem::Fn(method) = item else { return None };
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
    let is_pseudo_async = is_method_pseudo_async(method);
    let mut routes = Vec::new();
    let ident = &method.sig.ident;
    routes.push(method.sig.ident.to_string());

    for attr in &method.attrs {
        let syn::Meta::NameValue(x) = &attr.meta else { continue };
        let syn::Expr::Lit(syn::ExprLit { lit, .. }) = &x.value else {
            emit_error!(attr, "unexpected attribute");
            continue;
        };
        let syn::Lit::Str(route) = lit else {
            emit_error!(lit, "unexpected non-string literal attribute");
            continue;
        };

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
            quote!(__service.register_notify_handler(#route_paths, |__req: #tup_inputs| {
                T::#ident(#unpack);
                Ok(())
            }))
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
                    })
                )
            } else {
                quote!(
                    __service.register_request_handler(#route_paths, |__req: #tup_inputs, __rep: #treq| {
                        T::#ident(#unpack, __rep);
                        Ok(())
                    })
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
                })
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
                    })
                )
            } else {
                quote!(
                    let __this_2 = __this.clone();
                    __service.register_request_handler(#route_paths, move |__req: #tup_inputs, __rep: #treq| {
                        T::#ident(#this_param, #unpack, __rep);
                        Ok(())
                    })
                )
            }
        };
        LoaderOutput::Stateful(strm)
    })
}

fn generate_call_stubs(item: &TraitItem) -> Option<TokenStream> {
    None
}

fn generate_trait_signatures(items: &[TraitItem]) -> TokenStream {
    let tokens = items.iter().map(|item| {
        let TraitItem::Fn(method) = item else { return Cow::Borrowed(item) };
        let is_pseudo_async_method = is_method_pseudo_async(method);

        let mut method = method.clone();
        method.attrs.clear();

        // Find 'async' attribute

        // XXX: When static async method is stabilized, add support for it.
        // - For now, to not corrupt the async keyword usage, we only allow declaring pseudo-async
        //   method through method attribute
        if !is_pseudo_async_method {
            return Cow::Owned(TraitItem::Fn(method));
        }

        if method.default.is_some() {
            abort!(item, "You can't provide default implementation for async handler");
        }

        let outputs = OutputType::new(&method.sig.output);
        if outputs.is_notify() {
            abort!(item, "Only request handler can be async!");
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
        Cow::Owned(TraitItem::Fn(method))
    });

    quote!(#(#tokens)*)
}

fn is_method_pseudo_async(method: &TraitItemFn) -> bool {
    let mut is_async = false;
    for attr in &method.attrs {
        let syn::Meta::Path(path) = &attr.meta else { continue };
        if path.is_ident("async_fn") {
            is_async = true;
            break;
        }
    }
    is_async
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

    fn gen_client_recv(&self) -> TokenStream {
        assert!(!self.is_notify());

        todo!()
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
