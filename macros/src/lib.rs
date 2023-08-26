use convert_case::{Case, Casing};
use quote::quote;
use syn::{Token, VisRestricted};

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

    let ast = syn::parse2::<syn::ItemTrait>(tokens).unwrap();
    let module_name = format!("{}", ast.ident.to_string().to_case(Case::Snake));
    let module_name = syn::Ident::new(&module_name, proc_macro2::Span::call_site());
    let vis = match ast.vis.clone() {
        vis @ (syn::Visibility::Public(_) | syn::Visibility::Restricted(_)) => vis,
        syn::Visibility::Inherited => syn::Visibility::Restricted(VisRestricted {
            pub_token: Token![pub](proc_macro2::Span::call_site()),
            paren_token: Default::default(),
            in_token: None,
            path: Box::new(syn::parse2::<syn::Path>(quote!(self)).unwrap()),
        }),
    };

    let items = &ast.items;

    let output = quote!(
        mod #module_name {
            use rpc_it::service as __sv;
            use rpc_it::service::macro_utils as __mc;
            use std::sync as __sc;
            use rpc_it::serde;

            #vis trait Service {
                #(#items)*
            }

            #vis struct Loader;
            impl Loader {
                #vis fn load_stateful<T:Service>(
                    __this: __sc::Arc<T>,
                    __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>
                ) -> __mc::RegisterResult {
                    todo!("register signatures")
                }

                #vis fn load_stateless<T:Service>(
                    __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>
                ) -> __mc::RegisterResult {
                    todo!("register signatures")
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
                // TODO: declare call signatures

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

struct GenContext {
    module_vis: syn::Visibility,
    trait_name: syn::Ident,
}
