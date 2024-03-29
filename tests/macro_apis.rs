#![cfg(all(feature = "in-memory-io", feature = "proc-macro"))]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use futures::{executor::LocalPool, FutureExt};
use rpc_it::{io::InMemoryRx, Codec, DefaultConfig};

mod shared;

#[rpc_it::service(rename_all = "camelCase")]
mod rpc {
    use enum_as_inner::EnumAsInner;

    #[install]
    #[derive(EnumAsInner)]
    pub(super) const Route: Route = ALL_PASCAL_CASE;

    #[direct]
    pub const MainRoute: Route = [
        zero_param,
        two_param_add,
        three_param_concat,
        four_param_concat,
    ];

    #[direct]
    pub const SubRoute: Route = [one_param_flip, one_param_fp];

    #[direct]
    #[derive(EnumAsInner)]
    pub const Noti: Route = [
        noti_zero_param,
        noti_one_param,
        noti_two_params,
        noti_three_params,
    ];

    pub fn zero_param() -> i32;

    #[name = "One/Param/Flip"]
    pub(super) fn one_param_flip(a: i32) -> i32;

    pub fn pass_positive_value_only(x: i32) -> Result<i32, &str>;

    #[name = "T wo P aaa ram Ad"]
    pub fn two_param_add(a: i32, b: i32) -> i32;

    #[route = "Param/Three"]
    #[route = "Param/Thre"]
    #[route = "Param/Thr"]
    #[route = "Param/Th"]
    pub fn three_param_concat(a: &str, b: &str, c: &str) -> &str;

    pub fn four_param_concat(a: &str, b: i32, c: u64, d: &str) -> &str;

    pub fn one_param_fp(x: f32) -> f32;

    pub fn noti_zero_param();
    pub fn noti_one_param(st: &str);
    pub fn noti_two_params(st: &str, x: i32);
    pub fn noti_three_params(st: &str, x: i32, y: f64);
}

// Recursive expansion of service macro
// =====================================

type TestCfg<C> = DefaultConfig<Arc<DropCheck>, C>;

#[derive(Debug, Clone)]
struct DropCheck;

static DROP_CALLED: AtomicBool = AtomicBool::new(false);

impl Drop for DropCheck {
    fn drop(&mut self) {
        DROP_CALLED.store(true, Ordering::SeqCst);
    }
}

#[test]
#[cfg(feature = "jsonrpc")]
fn jsonrpc() {
    use rpc_it::ext_codec::jsonrpc;

    run_with_codec(|| jsonrpc::Codec);
}

#[test]
#[cfg(feature = "rawrpc")]
fn rawrpc() {
    use rpc_it::ext_codec::rawrpc;

    run_with_codec(|| rawrpc::Codec);
}

#[test]
#[cfg(feature = "msgpack-rpc")]
fn mspgack_rpc() {
    use rpc_it::ext_codec::msgpack_rpc;

    run_with_codec(msgpack_rpc::Codec::default)
}

fn run_with_codec<C: Codec>(codec: impl Fn() -> C) {
    let drop_check = Arc::new(DropCheck);

    {
        let mut executor = LocalPool::new();

        let (client, mut server) = shared::create_default_rpc_pair::<TestCfg<_>>(
            &executor.spawner(),
            drop_check.clone(),
            drop_check.clone(),
            codec,
        );

        executor.run_until(run_macro_ops_correct(&mut server, client.clone()));
        executor.run_until(run_notify_param_correct(&mut server, client.clone()));
    }

    drop(drop_check);
    assert!(DROP_CALLED.load(Ordering::SeqCst));
}

async fn run_notify_param_correct<C: Codec>(
    server: &mut rpc_it::Receiver<TestCfg<C>, InMemoryRx>,
    client: rpc_it::RequestSender<TestCfg<C>>,
) {
    let task_client = async {
        let b = &mut Default::default();

        client.notify(b, rpc::noti_zero_param()).await?;
        client.notify(b, rpc::noti_one_param("a")).await?;
        client.notify(b, rpc::noti_two_params("a", &1)).await?;
        client
            .notify(b, rpc::noti_three_params("a", &1, &2.0))
            .await?;

        Ok::<_, anyhow::Error>(())
    };

    let task_recv = async {
        macro_rules! get {
            () => {
                server
                    .recv()
                    .map(move |val| rpc::Noti::route(val.unwrap()))
                    .await
                    .ok()
                    .unwrap()
            };
        }

        get!().into_noti_zero_param().ok().unwrap();

        let arg = get!().into_noti_one_param().ok().unwrap();
        assert_eq!("a", *arg.args());

        let arg = get!().into_noti_two_params().ok().unwrap();
        assert_eq!("a", arg.args().st);
        assert_eq!(1, arg.args().x);

        let arg = get!().into_noti_three_params().ok().unwrap();
        assert_eq!("a", arg.args().st);
        assert_eq!(1, arg.args().x);
        assert_eq!(2.0, arg.args().y);

        Ok::<_, anyhow::Error>(())
    };

    let (r1, r2) = futures::join!(task_client, task_recv);
    r1.unwrap();
    r2.unwrap();
}

async fn run_macro_ops_correct<C: Codec>(
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

    let task_client = async {
        let b = &mut Default::default();

        let res_zero_param = client.request(b, rpc::zero_param()).await?;
        let res_one_param_flip = client.request(b, rpc::one_param_flip(&2)).await?;
        let res_pass_positive_value_only = client
            .request(b, rpc::pass_positive_value_only(&-5))
            .await?;
        let res_two_param_add = client.request(b, rpc::two_param_add(&1, &2)).await?;
        let res_three_param_concat = client
            .request(b, rpc::three_param_concat("a", "b", "c"))
            .await?;
        let res_four_param_concat = client
            .request(b, rpc::four_param_concat("abc", &1, &23, "abc"))
            .await?;
        let res_one_param_fp = client.request(b, rpc::one_param_fp(&1.55)).await?;

        let (
            res_zero_param,
            res_two_param_add,
            res_one_param_flip,
            res_three_param_concat,
            res_four_param_concat,
            res_one_param_fp,
            res_pass_positive_value_only,
        ) = futures::join!(
            res_zero_param,
            res_two_param_add,
            res_one_param_flip,
            res_three_param_concat,
            res_four_param_concat,
            res_one_param_fp,
            res_pass_positive_value_only,
        );

        assert_eq!(3, *res_zero_param?.value());
        assert_eq!(-2, *res_one_param_flip?.value());
        assert_eq!(
            "-5 is Negative Value",
            *res_pass_positive_value_only
                .map(drop)
                .unwrap_err()
                .into_response()
                .unwrap()
                .value()
        );
        assert_eq!(3, *res_two_param_add?.value());
        assert_eq!("abc", *res_three_param_concat?.value());
        assert_eq!("abc123abc", *res_four_param_concat?.value());
        assert_eq!(-1.0, *res_one_param_fp?.value());

        Ok::<_, anyhow::Error>(())
    };

    let task_service = async {
        let b = &mut Default::default();

        let msg = rx_route.recv().await?.into_zero_param().ok().unwrap();
        msg.respond(b, Ok(&3)).await?;

        let msg = rx_route.recv().await?.into_one_param_flip().ok().unwrap();
        msg.respond(b, Ok(&-*msg.args())).await?;

        let msg = rx_route
            .recv()
            .await?
            .into_pass_positive_value_only()
            .ok()
            .unwrap();
        msg.respond(b, Err(&format!("{} is Negative Value", *msg.args())))
            .await?;

        let msg = rx_route.recv().await?.into_two_param_add().ok().unwrap();
        msg.respond(b, Ok(&(msg.args().a + msg.args().b))).await?;

        let msg = rx_route
            .recv()
            .await?
            .into_three_param_concat()
            .ok()
            .unwrap();
        msg.respond(
            b,
            Ok(&format!("{}{}{}", msg.args().a, msg.args().b, msg.args().c)),
        )
        .await?;

        let msg = rx_route
            .recv()
            .await?
            .into_four_param_concat()
            .ok()
            .unwrap();
        let arg = msg.args();
        msg.respond(b, Ok(&format!("{}{}{}{}", arg.a, arg.b, arg.c, arg.d,)))
            .await?;

        let msg = rx_route.recv().await?.into_one_param_fp().ok().unwrap();
        msg.respond(b, Ok(&(-msg.args().floor() as _))).await?;

        Ok::<_, anyhow::Error>(())
    };

    let task_bg_recv = async {
        let router = route_bulider.finish();
        while let Ok(rx) = server.recv().await {
            router.route(rx).unwrap();
        }
    };

    let (r1, r2) = futures::join!(task_client, async {
        futures::select! {
            r = task_service.fuse() => r,
            _ = task_bg_recv.fuse() => panic!("bg recv task should not finish"),
        }
    });

    r1.unwrap();
    r2.unwrap();
}
