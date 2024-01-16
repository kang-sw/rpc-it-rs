#![cfg(all(feature = "in-memory-io", feature = "proc-macro"))]

#[rpc_it::service]
mod rpc {
    pub fn zero_param() -> i32;

    pub fn one_param_flip(a: i32) -> i32;

    pub fn two_param_add(a: i32, b: i32) -> i32;

    pub fn three_param_concat(a: String, b: String, c: String) -> String;

    pub fn four_param_concat(a: String, b: i32, c: u64, d: String) -> String;

    pub fn one_param_split(x: __<i32, f32>) -> i32;
}

#[cfg(test)]
fn test() {}

async fn test_param_delivery() {}
