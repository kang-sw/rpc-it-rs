use rpc_it::{ExactMatchRouter, ServiceBuilder};
use rpc_it_macros::service;
use std::sync::Arc;
pub(crate) mod my_service {
    #![allow(unused_parens)]
    use rpc_it::serde;
    use rpc_it::service as __sv;
    use rpc_it::service::macro_utils as __mc;
    use std::sync as __sc;
    pub(crate) trait Service: Send + Sync + 'static {
        fn add(a: i32, b: i32) -> i32 {
            a + b
        }
        fn div(a: i32, b: i32) -> Result<i32, String>;
        fn neg(v: i32) -> i32 {
            -v
        }
        fn notify_mono(a: i32) {}
        fn notify_zero() {}
        fn notify(a: i32, b: i32);
        fn notify_my_name(a: &str, b: &str);
        fn amplify(&self, a: i32, b: i32) -> i32 {
            a + b
        }
        fn amplify_2(&self, a: i32, b: i32, _: rpc_it::TypedRequest<i32, ()>);
        fn amplify_3(self: &std::sync::Arc<Self>, a: i32, b: i32, _: rpc_it::TypedRequest<i32, ()>);
        fn introduce(&self) -> i32 {
            0
        }
    }
    pub(crate) struct Loader;
    impl Loader {
        pub(crate) fn load_stateful<T: Service, R: __sv::Router>(
            __this: __sc::Arc<T>,
            __service: &mut __sv::ServiceBuilder<R>,
        ) -> __mc::RegisterResult {
            let __this_2 = __this.clone();
            __service.register_request_handler(
                &["amplify"],
                move |__req: (i32, i32), __rep: rpc_it::TypedRequest<i32, ()>| {
                    let __result = T::amplify(&__this_2, __req.0, __req.1);
                    __rep.ok(&__result)?;
                    Ok(())
                },
            );
            let __this_2 = __this.clone();
            __service.register_request_handler(
                &["amplify_2"],
                move |__req: (i32, i32), __rep: rpc_it::TypedRequest<i32, ()>| {
                    T::amplify_2(&__this_2, __req.0, __req.1, __rep);
                    Ok(())
                },
            );
            let __this_2 = __this.clone();
            __service.register_request_handler(
                &["amplify_3"],
                move |__req: (i32, i32), __rep: rpc_it::TypedRequest<i32, ()>| {
                    T::amplify_3(&__this_2, __req.0, __req.1, __rep);
                    Ok(())
                },
            );
            let __this_2 = __this.clone();
            __service.register_request_handler(
                &["introduce"],
                move |__req: (), __rep: rpc_it::TypedRequest<i32, ()>| {
                    let __result = T::introduce(&__this_2);
                    __rep.ok(&__result)?;
                    Ok(())
                },
            );
            Ok(())
        }
        pub(crate) fn load_stateless<'__l_a, T: Service>(
            __service: &'__l_a mut __sv::ServiceBuilder<__sv::ExactMatchRouter>,
        ) -> __mc::RegisterResult {
            __service.register_request_handler(
                &["add"],
                |__req: (i32, i32), __rep: rpc_it::TypedRequest<i32, ()>| {
                    let __result = T::add(__req.0, __req.1);
                    __rep.ok(&__result)?;
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
            __service.register_request_handler(
                &["neg"],
                |__req: (i32), __rep: rpc_it::TypedRequest<i32, ()>| {
                    let __result = T::neg(__req);
                    __rep.ok(&__result)?;
                    Ok(())
                },
            );
            __service.register_notify_handler(&["notify_mono"], |__req: (i32)| {
                T::notify_mono(__req);
                Ok(())
            });
            __service.register_notify_handler(&["notify_zero"], |__req: ()| {
                T::notify_zero();
                Ok(())
            });
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
        pub(crate) fn load<T: Service>(
            __this: T,
            __service: &mut __sv::ServiceBuilder<__sv::ExactMatchRouter>,
        ) -> __mc::RegisterResult {
            Self::load_stateful(__sc::Arc::new(__this), __service)
                .and_then(|_| Self::load_stateless::<T>(__service))
        }
    }
    pub(crate) struct Stub<'a>(std::borrow::Cow<'a, rpc_it::Transceiver>);
    impl<'a> Stub<'a> {
        pub(crate) async fn add(
            &self,
            a: &i32,
            b: &i32,
        ) -> Result<i32, rpc_it::TypedCallError<()>> {
            self.0.call_with_err("add", &(a, b)).await
        }
        pub(crate) async fn div(
            &self,
            a: &i32,
            b: &i32,
        ) -> Result<i32, rpc_it::TypedCallError<String>> {
            self.0.call_with_err("div", &(a, b)).await
        }
        pub(crate) async fn neg(&self, v: &i32) -> Result<i32, rpc_it::TypedCallError<()>> {
            self.0.call_with_err("neg", &(v)).await
        }
        pub(crate) async fn notify_mono(&self, a: &i32) -> Result<(), rpc_it::SendError> {
            self.0.notify("notify_mono", &(a)).await
        }
        pub(crate) async fn notify_mono_deferred(&self, a: &i32) -> Result<(), rpc_it::SendError> {
            self.0.notify_deferred("notify_mono", &(a))
        }
        pub(crate) async fn notify_zero(&self) -> Result<(), rpc_it::SendError> {
            self.0.notify("notify_zero", &()).await
        }
        pub(crate) async fn notify_zero_deferred(&self) -> Result<(), rpc_it::SendError> {
            self.0.notify_deferred("notify_zero", &())
        }
        pub(crate) async fn notify(&self, a: &i32, b: &i32) -> Result<(), rpc_it::SendError> {
            self.0.notify("notify", &(a, b)).await
        }
        pub(crate) async fn notify_deferred(
            &self,
            a: &i32,
            b: &i32,
        ) -> Result<(), rpc_it::SendError> {
            self.0.notify_deferred("notify", &(a, b))
        }
        pub(crate) async fn notify_my_name(
            &self,
            a: &str,
            b: &str,
        ) -> Result<(), rpc_it::SendError> {
            self.0.notify("notify_my_name", &(a, b)).await
        }
        pub(crate) async fn notify_my_name_deferred(
            &self,
            a: &str,
            b: &str,
        ) -> Result<(), rpc_it::SendError> {
            self.0.notify_deferred("notify_my_name", &(a, b))
        }
        #[doc(hidden)]
        pub(crate) async fn notify_my_name_with_reuse(
            &self,
            buffer: &mut rpc_it::rpc::WriteBuffer,
            a: &str,
            b: &str,
        ) -> Result<(), rpc_it::SendError> {
            self.0.notify_with_reuse(buffer, "notify_my_name", &(a, b)).await
        }
        #[doc(hidden)]
        pub(crate) async fn notify_my_name_deferred_with_reuse(
            &self,
            buffer: &mut rpc_it::rpc::WriteBuffer,
            a: &str,
            b: &str,
        ) -> Result<(), rpc_it::SendError> {
            self.0.notify_deferred_with_reuse(buffer, "notify_my_name", &(a, b))
        }
        pub(crate) async fn amplify(
            &self,
            a: &i32,
            b: &i32,
        ) -> Result<i32, rpc_it::TypedCallError<()>> {
            self.0.call_with_err("amplify", &(a, b)).await
        }
        pub(crate) async fn amplify_2(
            &self,
            a: &i32,
            b: &i32,
        ) -> Result<i32, rpc_it::TypedCallError<()>> {
            self.0.call_with_err("amplify_2", &(a, b)).await
        }
        pub(crate) async fn amplify_3(
            &self,
            a: &i32,
            b: &i32,
        ) -> Result<i32, rpc_it::TypedCallError<()>> {
            self.0.call_with_err("amplify_3", &(a, b)).await
        }
        pub(crate) async fn introduce(&self) -> Result<i32, rpc_it::TypedCallError<()>> {
            self.0.call_with_err("introduce", &()).await
        }
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
