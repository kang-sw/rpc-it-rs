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

    async fn amplify_2(&self, a: i32, b: i32) -> i32;

    fn introduce(&self) -> i32 {
        0
    }
}

fn foo() {
    let mut svc: ServiceBuilder<ExactMatchRouter> = todo!();
    svc.register_notify_handler(&["notify_my_name"], |__req: (&str, &str)| Ok(()));
}
