#![cfg(all(feature = "in-memory-io", feature = "proc-macro"))]

use rpc_it::{io::InMemoryRx, Codec, DefaultConfig};

#[rpc_it::service(rename_all = "camelCase")]
mod rpc {
    pub const Route: Route = ALL_PASCAL_CASE;

    #[direct]
    #[no_install]
    pub const DRoute: Route = ALL_PASCAL_CASE;

    pub fn zero_param() -> i32;

    #[name = "One/Param/Flip"]
    pub fn one_param_flip(a: i32) -> i32;

    #[name = "T wo P aaa ram Ad"]
    pub fn two_param_add(a: i32, b: i32) -> i32;

    #[route = "Param/Three"]
    #[route = "Param/Thre"]
    #[route = "Param/Thr"]
    #[route = "Param/Th"]
    pub fn three_param_concat(a: String, b: String, c: String) -> String;

    pub fn four_param_concat(a: String, b: i32, c: u64, d: String) -> String;

    pub fn one_param_split(x: __<i32, f32>) -> i32;
}

// Recursive expansion of service macro
// =====================================

type CodecCfg<C> = DefaultConfig<(), C>;

#[test]
fn test_macro_ops_correct_jsonrpc() {}

async fn test_macro_ops_correct<C: Codec>(
    server: &mut rpc_it::Receiver<CodecCfg<C>, InMemoryRx>,
    client: rpc_it::RequestSender<CodecCfg<C>>,
) {
    let mut route_bulider = rpc_it::router::StdHashMapBuilder::default();
    let (tx_route, rx_route) = mpsc::unbounded();
    rpc::Route::<CodecCfg<C>>::install(
        &mut route_bulider,
        move |x| {
            tx_route.try_send(x).unwrap();
        },
        |_, _, _| {},
    );

    let router = route_bulider.finish();
}
