use rpc_it::{ExactMatchRouter, ServiceBuilder};
use rpc_it_macros::service;
mod my_service {
    #![allow(unused_parens)]
    use rpc_it::serde;
    use rpc_it::service as __sv;
    use rpc_it::service::macro_utils as __mc;
    use std::sync as __sc;
    pub trait Service {
        fn add(a: i32, b: i32) -> i32 {
            a + b
        }
        fn div(a: i32, b: i32) -> Result<i32, String>;
        fn notify(a: i32, b: i32);
        fn notify_my_name(a: &str, b: &str);
        fn amplify(&self, a: i32, b: i32) -> i32 {
            a + b
        }
        fn amplify_2(&self, a: i32, b: i32, _: rpc_it::TypedRequest<i32, ()>) -> i32;
        fn introduce(&self) -> i32 {
            0
        }
    }
    pub struct Loader;
    impl Loader {
        pub fn load_stateful<T: Service, R: __sv::Router>(
            __this: __sc::Arc<T>,
            __service: &mut __sv::ServiceBuilder<R>,
        ) -> __mc::RegisterResult {
            Ok(())
        }
        pub fn load_stateless<'__l_a, T: Service>(
            __service: &'__l_a mut __sv::ServiceBuilder<__sv::ExactMatchRouter>,
        ) -> __mc::RegisterResult {
            __service.register_request_handler(
                &["add"],
                |__req: (i32, i32), __rep: rpc_it::TypedRequest<i32, ()>| {
                    let __result = T::add(__req.0, __req.1);
                    __rep.ok(&__result);
                    Ok(())
                },
            );
            __service.register_request_handler(
                &["div"],
                |__req: (i32, i32), __rep: rpc_it::TypedRequest<i32, String>| {
                    let __result = T::div(__req.0, __req.1);
                    match __result {
                        Ok(x) => __rep.ok(&x)?,
                        Err(e) => __rep.err(&e)?,
                    };
                    Ok(())
                },
            );
            __service.register_notify_handler(
                &["notify", "hello", "hello2"],
                |__req: (i32, i32)| {
                    T::notify(__req.0, __req.1);
                    Ok(())
                },
            );
            __service.register_notify_handler(
                &["notify_my_name"],
                |__req: (std::borrow::Cow<str>, std::borrow::Cow<str>)| {
                    T::notify_my_name(&__req.0, &__req.1);
                    Ok(())
                },
            );
            Ok(())
        }
        pub fn load<T: Service>(
            __this: T,
            __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>,
        ) -> __mc::RegisterResult {
            Self::load_stateful(__sc::Arc::new(__this), __service)
                .and_then(|_| Self::load_stateless::<T>(__service))
        }
    }
    pub struct Stub<'a>(std::borrow::Cow<'a, rpc_it::Transceiver>);
    impl<'a> Stub<'a> {
        async fn __call<Req, Rep, Err>(
            &self,
            method: &str,
            req: &Req,
        ) -> Result<Rep, rpc_it::TypedCallError<Err>>
        where
            Req: serde::Serialize,
            Rep: serde::de::DeserializeOwned,
            Err: serde::de::DeserializeOwned,
        {
            self.0.call_with_err(method, req).await
        }
    }
}
