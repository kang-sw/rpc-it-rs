#[rpc_it_macros::service]
extern "hello" {
    // If return type is specified explicitly, it is treated as request.
    pub fn method_req(arg: (i32, i32)) -> ();
}

pub struct MyParam<'a> {
    name: &'a str,
    age: &'a str,
}
