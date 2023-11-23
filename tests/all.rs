use std::{future::Future, time::Instant};

use futures_util::join;
use rpc_it::{
    codec::Codec,
    kv_pairs,
    rpc::{CallError, MessageMethodName},
    Message, RecvMsg, Transceiver,
};

/* ---------------------------------------------------------------------------------------------- */
/*                                         TEST INSTANCES                                         */
/* ---------------------------------------------------------------------------------------------- */

#[cfg(all(feature = "in-memory", feature = "jsonrpc"))]
#[tokio::test]
async fn test_basic_io_jsonrpc() {
    basic_io_test(rpc_it::codecs::jsonrpc::Codec::default, |s, c| {
        request_test(s, c.into_sender(), false)
    })
    .await;
}

#[cfg(all(feature = "in-memory", feature = "msgpack-rpc"))]
#[tokio::test]
async fn test_basic_io_msgpack_rpc() {
    basic_io_test(
        || {
            rpc_it::codecs::msgpack_rpc::Codec::default()
                .with_auto_wrapping(true)
                .with_unwrap_mono_param(true)
        },
        |s, c| request_test(s, c.into_sender(), true),
    )
    .await;
}

/* ---------------------------------------------------------------------------------------------- */
/*                                          TEST METHODS                                         */
/* ---------------------------------------------------------------------------------------------- */

async fn request_test(
    server: rpc_it::Transceiver,
    client: rpc_it::Sender,
    support_predef_err: bool,
) {
    #[allow(unused)]
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct DropMeDe {
        a: i32,
        b: i32,
        c: i32,
        e: DropMeDeInner,
    }

    #[allow(unused)]
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct DropMeDeInner {
        f: i32,
        g: i32,
        h: Vec<i32>,
        k: [String; 2],
    }

    let task_server = async move {
        while let Ok(msg) = server.recv().await {
            let RecvMsg::Request(req) = msg else { unreachable!() };

            let method_name = req.method().expect("method name is invalid utf-8");
            if method_name.starts_with("add-") {
                let [a, b] = req.parse::<[i32; 2]>().expect("parse failed");

                assert_eq!(method_name, format!("add-{a}-plus-{b}"));
                req.response(Ok(a + b).as_ref()).await.expect("response failed");
            } else if method_name.starts_with("drop-me") {
                assert_eq!(
                    req.parse::<DropMeDe>().expect("parse failed"),
                    DropMeDe {
                        a: 1,
                        b: 3,
                        c: 4,
                        e: DropMeDeInner {
                            f: 5,
                            g: 6,
                            h: vec![7, 8, 9],
                            k: ["alpha".into(), "beta".into()]
                        },
                    }
                );
                drop(req);
            }
        }
    };

    let task_client = async move {
        let start_at = Instant::now();
        const N_CALLS: i32 = 40000;
        for i in 0..N_CALLS {
            match i % 2 {
                0 => {
                    // Verify 'Add' operation
                    let a = i * 1000;
                    let b = a + 2000;
                    let method_name = format!("add-{a}-plus-{b}");
                    // println!("sending request: {}", method_name);

                    let req = client.request(&method_name, &(a, b)).await;
                    // println!("req sent");

                    let resp = req
                        .expect("request failed")
                        .to_owned()
                        .await
                        .expect("receiving response failed");

                    let value = resp.parse::<i32>().expect("parsing retunred value failed");
                    assert_eq!(value, a + b);

                    let value = resp.result::<i32, ()>().unwrap();
                    assert_eq!(value, a + b);
                }
                1 => {
                    // Verify 'Dropped' error
                    let ident_k = "k";
                    let req = client
                        .call::<()>(
                            "drop-me",
                            &kv_pairs!(
                                "a" = 1,
                                "b" = 3,
                                "c" = 4,
                                "e" = kv_pairs!(
                                    "f" = 5,
                                    "g" = 6,
                                    "h" = [7, 8, 9],
                                    ident_k = ["alpha", "beta"]
                                )
                            ),
                        )
                        .await;

                    let CallError::ErrorResponse(resp) = req.expect_err("request failed") else {
                        unreachable!()
                    };

                    assert!(resp.is_error());
                    let err = resp.result::<(), ()>().unwrap_err();

                    if support_predef_err {
                        assert!(err.into_predefined().unwrap().is_unhandled());
                    } else {
                        assert!(err.is_decode_error());
                    }
                }
                _ => unreachable!(),
            }
        }
        client.close();
        println!(
            "elapsed: {:?}, iter: {:?}",
            start_at.elapsed(),
            start_at.elapsed() / N_CALLS as u32
        );
    };

    join!(task_server, task_client);
}

async fn broadcast_test(publisher: Transceiver, receiver: Transceiver) {
    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(untagged)]
    enum Scenario {
        Integer(i32),
        String(String),
        Boolean(bool),
        Object(Subscenario),
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    enum Subscenario {
        Nodata,
        Nodata2,
        DataInteger(i32, u64, i128),
        DatTup(i32, f64, f32, [String; 3]),
        DataStruct { a: i32, b: i32, c: i32 },
    }

    let scenarios = vec![
        Scenario::Integer(42),
        Scenario::String("Hello, world!".to_string()),
        Scenario::Boolean(true),
        Scenario::Object(Subscenario::Nodata),
        Scenario::Object(Subscenario::Nodata2),
        Scenario::Object(Subscenario::DataInteger(10, 100, 1000)),
        Scenario::Object(Subscenario::DatTup(
            1,
            3.14,
            2.718,
            ["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        )),
        Scenario::Object(Subscenario::DataStruct { a: 1, b: 2, c: 3 }),
    ];

    todo!("BROADCAST FEATURE TEST");
    todo!("REUSABLE NOTIFICAITON MESSAGE TEST");
}

/* ---------------------------------------------------------------------------------------------- */
/*                                         TEST UTILITIES                                         */
/* ---------------------------------------------------------------------------------------------- */

async fn basic_io_test<T: Codec, F: Future<Output = ()>>(
    create_codec: impl Fn() -> T,
    test_peers: impl FnOnce(Transceiver, Transceiver) -> F,
) {
    let (tx_server, rx_client) = rpc_it::transports::new_in_memory();
    let (tx_client, rx_server) = rpc_it::transports::new_in_memory();

    let (server, task) = rpc_it::Builder::default()
        .with_read(rx_server)
        .with_write(tx_server)
        .with_event_listener(LoggingSubscriber("server"))
        .with_feature(rpc_it::Feature::ENABLE_AUTO_RESPONSE)
        .with_codec(create_codec())
        .build();

    tokio::spawn(task);

    let (client, task) = rpc_it::Builder::default()
        .with_read(rx_client)
        .with_write(tx_client)
        .with_event_listener(LoggingSubscriber("client"))
        .with_request()
        .with_codec(create_codec())
        .build();

    tokio::spawn(task);

    // request_test(server, client.into_sender(), supports_predef).await;
    test_peers(server, client).await;
}

struct LoggingSubscriber(&'static str);
impl rpc_it::InboundEventSubscriber for LoggingSubscriber {
    fn on_close(&self, closed_by_us: bool, result: std::io::Result<()>) {
        println!("[{}] closing: closed-by-us={closed_by_us}, result={result:?}", self.0);
    }

    fn on_inbound_error(&self, error: rpc_it::InboundError) {
        println!("{}", std::backtrace::Backtrace::capture());
        println!("[{}] inbound error: {:?}", self.0, error);
    }
}
