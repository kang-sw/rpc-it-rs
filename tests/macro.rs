#![cfg(feature = "macros")]

#[rpc_it::service]
mod test_api {
    #[request]
    fn my_method(my_bin: &[u8]) -> bool;
}
