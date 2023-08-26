use std::sync::Arc;

use rpc_it::{ExactMatchRouter, ServiceBuilder};
use rpc_it_macros::service;

#[service]
pub trait MyService {
    fn add(a: i32, b: i32) -> i32 {
        a + b
    }

    fn div(a: i32, b: i32) -> Result<i32, String>;

    #[routes = "hello"]
    #[routes = "hello2"]
    fn notify(a: i32, b: i32);

    fn notify_my_name(a: &str, b: &str);

    fn amplify(&self, a: i32, b: i32) -> i32 {
        a + b
    }

    #[async_fn]
    fn amplify_2(&self, a: i32, b: i32) -> i32;

    #[async_fn]
    fn amplify_3(self: &std::sync::Arc<Self>, a: i32, b: i32) -> i32;

    fn introduce(&self) -> i32 {
        0
    }
}

struct MyServiceImpl;

impl my_service::Service for MyServiceImpl {
    fn div(a: i32, b: i32) -> Result<i32, String> {
        todo!()
    }

    fn notify(a: i32, b: i32) {
        todo!()
    }

    fn notify_my_name(a: &str, b: &str) {
        todo!()
    }

    fn amplify_2(&self, a: i32, b: i32, _: rpc_it::TypedRequest<i32, ()>) {
        todo!()
    }

    fn amplify_3(self: &std::sync::Arc<Self>, a: i32, b: i32, _: rpc_it::TypedRequest<i32, ()>) {
        todo!()
    }
}

fn foo() {
    let mut svc: ServiceBuilder<ExactMatchRouter> = todo!();
    svc.register_notify_handler(&["notify_my_name"], |__req: (&str, &str)| Ok(()));
}
