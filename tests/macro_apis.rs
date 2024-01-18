#![cfg(all(feature = "in-memory-io", feature = "proc-macro"))]

use futures::executor::LocalPool;
use rpc_it::{ext_codec::jsonrpc, io::InMemoryRx, Codec, DefaultConfig};

mod shared;

#[rpc_it::service(rename_all = "camelCase")]
mod rpc {
    #[install]
    pub const Route: Route = ALL_PASCAL_CASE;

    #[direct]
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

type TestCfg<C> = DefaultConfig<(), C>;

#[test]
fn test_macro_ops_correct_jsonrpc() {
    let mut executor = LocalPool::new();

    let (client, mut server) = shared::create_default_rpc_pair::<TestCfg<jsonrpc::Codec>>(
        &executor.spawner(),
        (),
        (),
        || jsonrpc::Codec,
    );

    executor.run_until(test_macro_ops_correct(&mut server, client.clone()));
}

async fn test_macro_ops_correct<C: Codec>(
    server: &mut rpc_it::Receiver<TestCfg<C>, InMemoryRx>,
    client: rpc_it::RequestSender<TestCfg<C>>,
) {
    let mut route_bulider = rpc_it::router::StdHashMapBuilder::default();
    let (tx_route, rx_route) = mpsc::unbounded();
    rpc::Route::<TestCfg<C>>::install(
        &mut route_bulider,
        move |x| {
            tx_route.try_send(x).unwrap();
        },
        |_, _, _| {},
    );

    let router = route_bulider.finish();

    let task_client = async {
        let b = &mut Default::default();
        assert_eq!(3, *client.try_call(b, rpc::zero_param())?.await?.result());

        assert_eq!(
            -2,
            *client.try_call(b, rpc::one_param_flip(&2))?.await?.result()
        );

        Ok::<_, anyhow::Error>(())
    };

    let task_service = async {
        while let Ok(rx) = rx_route.recv().await {
            match rx {
                rpc::Route::ZeroParam(msg) => todo!(),
                rpc::Route::OneParamFlip(_) => todo!(),
                rpc::Route::TwoParamAdd(_) => todo!(),
                rpc::Route::ThreeParamConcat(_) => todo!(),
                rpc::Route::FourParamConcat(_) => todo!(),
                rpc::Route::OneParamSplit(_) => todo!(),
            }
        }
    };
}
