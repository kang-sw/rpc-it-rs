#![cfg(feature = "in-memory-io")]

use std::sync::Arc;

use bytes::BytesMut;
use futures::{executor::LocalPool, task::SpawnExt, StreamExt};
use rpc_it::{rpc::RpcConfig, ParseMessage, ResponseError};

use crate::shared::create_default_rpc_pair;

mod shared;

#[test]
#[cfg(feature = "jsonrpc")]
fn verify_request_jsonrpc() {
    use rpc_it::{ext_codec::jsonrpc, rpc::Rpc};

    verify_request::<Rpc<(), _>>(|| jsonrpc::Codec);
}

#[test]
#[cfg(all(feature = "jsonrpc", feature = "dynamic-codec"))]
fn verify_request_dynamic_codecs() {
    use rpc_it::{
        codec::{self, DynamicCodec},
        ext_codec::jsonrpc,
        rpc::Rpc,
    };

    verify_request::<Rpc<(), DynamicCodec>>(|| Arc::new(jsonrpc::Codec));
}

fn verify_request<R: RpcConfig>(codec: impl Fn() -> R::Codec)
where
    R::UserData: Default,
{
    let mut executor = LocalPool::new();

    let spawner = executor.spawner();
    let (client, server) = create_default_rpc_pair::<R>(
        &spawner,
        R::UserData::default(),
        R::UserData::default(),
        codec,
    );

    let test_counter = Arc::new(());

    // Test basic request ping-pong
    {
        let client = client.clone();
        let server = server.clone();
        let tc = test_counter.clone();

        spawner
            .spawn(async move {
                let b = &mut BytesMut::new();

                // Test plain request response
                println!("I.client: sending request");
                let response = client.request(b, "hello", &(1, 2)).await.unwrap();

                println!("I.server: receiving request");
                let request = server.recv().await.unwrap();

                assert_eq!(request.method(), "hello");
                let (x1, x2) = request.parse::<(i32, i32)>().unwrap();
                assert_eq!((x1, x2), (1, 2));

                println!("I.server: sending response");
                request.response(b, Ok(x1 + x2)).await.unwrap();

                println!("I.client: receiving response");
                let response = response.await.unwrap();

                assert_eq!(response.parse::<i32>().unwrap(), 3);

                // Test dropped request
                println!("I.client: sending request 2");
                let response = client.request(b, "hello", &(3, 4)).await.unwrap();

                println!("I.server: receiving request 2");
                let request = server.recv().await.unwrap();

                println!("I.server: dropping request 2");
                drop(request);

                println!("I.client: receiving response 2");
                let err = response.await.unwrap_err();
                let Some(err) = err else { panic!() };

                assert_eq!(err.errc(), ResponseError::Unhandled);

                println!("I: done");
                drop(tc);
            })
            .unwrap();
    }

    executor.run_until_stalled();

    {
        let client = client.clone();
        let tc = test_counter.clone();

        spawner
            .spawn(async move {
                let b = &mut BytesMut::new();
                let req1 = client.request(b, "pewpew", &(1, 2)).await.unwrap();
                let req2 = client.request(b, "tewtew", &(3, 4)).await.unwrap();
                let req3 = client.request(b, "rewrew", &(5, 6)).await.unwrap();
                let req4 = client.request(b, "qewqew", &(7, 8)).await.unwrap();

                let rep1 = req1.await.unwrap().parse::<i32>().unwrap();
                let rep2 = req2.await.unwrap().parse::<i32>().unwrap();
                let rep3 = req3.await.unwrap().parse::<i32>().unwrap();
                let rep4 = req4.await.unwrap_err().unwrap().parse::<i32>().unwrap();

                assert_eq!(rep1, 3);
                assert_eq!(rep2, 7);
                assert_eq!(rep3, 11);
                assert_eq!(rep4, 15);

                println!("II.client: done");
                drop(tc);
            })
            .unwrap();

        let server = server.clone();
        let tc = test_counter.clone();

        spawner
            .spawn(async move {
                let b = &mut BytesMut::new();

                let req1 = server.recv().await.unwrap();
                let req2 = server.recv().await.unwrap();
                let req3 = server.recv().await.unwrap();
                let req4 = server.recv().await.unwrap();

                assert_eq!(req1.method(), "pewpew");
                assert_eq!(req2.method(), "tewtew");
                assert_eq!(req3.method(), "rewrew");
                assert_eq!(req4.method(), "qewqew");

                let (x1, x2) = req1.parse::<(i32, i32)>().unwrap();
                let (x3, x4) = req2.parse::<(i32, i32)>().unwrap();
                let (x5, x6) = req3.parse::<(i32, i32)>().unwrap();
                let (x7, x8) = req4.parse::<(i32, i32)>().unwrap();

                assert_eq!((x1, x2), (1, 2));
                assert_eq!((x3, x4), (3, 4));
                assert_eq!((x5, x6), (5, 6));
                assert_eq!((x7, x8), (7, 8));

                req1.response(b, Ok(x1 + x2)).await.unwrap();
                req2.response(b, Ok(x3 + x4)).await.unwrap();
                req3.response(b, Ok(x5 + x6)).await.unwrap();
                req4.response(b, Err(x7 + x8)).await.unwrap();

                println!("II.server: done");
                drop(tc);
            })
            .unwrap();
    }

    executor.run_until_stalled();

    // Check if all tests were executed/passed
    assert_eq!(Arc::strong_count(&test_counter), 1);
}
