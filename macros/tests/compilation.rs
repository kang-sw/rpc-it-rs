use rpc_it_macros::service;

#[service]
pub trait MyService {
    fn add(a: i32, b: i32) -> i32 {
        a + b
    }

    fn div(a: i32, b: i32) -> Result<i32, String>;
}

fn foo() {}
