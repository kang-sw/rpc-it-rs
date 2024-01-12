#![cfg(all(feature = "jsonrpc", feature = "in-memory-io"))]

use std::{
    clone,
    sync::{atomic::AtomicUsize, Arc},
};

use bytes::BytesMut;
use futures::{
    executor::LocalPool,
    task::{Spawn, SpawnExt},
    StreamExt,
};
use rpc_it::{
    error::{ErrorResponse, ReceiveResponseError, SendMsgError},
    ext_codec::jsonrpc,
    ParseMessage, ResponseError,
};

#[test]
fn verify_notify() {
    let (tx, rx) = rpc_it::io::in_memory(1);

    let (tx_rpc, task_runner) = rpc_it::builder()
        .with_codec(jsonrpc::Codec)
        .with_frame_writer(tx)
        .with_outbound_queue_capacity(1)
        .build_write_only();

    let task_send = async {
        let mut buf = BytesMut::new();
        let b = &mut buf;

        tx_rpc
            .notify(b, "test", &serde_json::json!({ "test": "test" }))
            .await
            .unwrap();

        tx_rpc.notify(b, "t-e-st-23", &1234).await.unwrap();
        tx_rpc.notify(b, "close", &()).await.unwrap();

        tx_rpc.shutdown_writer(false).await.unwrap();

        let err = tx_rpc
            .notify(b, "this-should-fail", &3141)
            .await
            .unwrap_err();

        assert!(matches!(err, SendMsgError::ChannelClosed));
    };

    let task_recv = async {
        let mut rx = rx;

        let contents: &[&str] = &[
            r#"{"jsonrpc":"2.0","method":"test","params":{"test":"test"}}"#,
            r#"{"jsonrpc":"2.0","method":"t-e-st-23","params":1234}"#,
            r#"{"jsonrpc":"2.0","method":"close","params":null}"#,
        ];

        for content in contents {
            let msg = rx.next().await.unwrap();
            assert_eq!(msg, content);
        }

        assert!(rx.next().await.is_none());
    };

    futures::executor::block_on(async {
        let (r_w_task, ..) = futures::join!(task_runner, task_send, task_recv);

        assert!(matches!(
            r_w_task.unwrap(),
            rpc_it::error::WriteRunnerExitType::ManualClose
        ));
    });
}

fn create_default_rpc_pair<US, UC>(
    spawner: &impl Spawn,
    server_user_data: US,
    client_user_data: UC,
) -> (rpc_it::RequestSender<UC>, rpc_it::Receiver<US>)
where
    US: rpc_it::UserData,
    UC: rpc_it::UserData,
{
    let (tx_client, rx_server) = rpc_it::io::in_memory(1);
    let (tx_server, rx_client) = rpc_it::io::in_memory(1);

    let tx_rpc = {
        let (tx_rpc, task_1, task_2) = rpc_it::builder()
            .with_codec(jsonrpc::Codec)
            .with_frame_writer(tx_client)
            .with_frame_reader(rx_client)
            .with_user_data(client_user_data)
            .with_inbound_queue_capacity(1)
            .with_outbound_queue_capacity(1)
            .build_client();

        spawner
            .spawn(async move {
                task_1.await.ok();
            })
            .unwrap();
        spawner
            .spawn(async move {
                task_2.await.ok(); // TODO: assert error
            })
            .unwrap();

        tx_rpc
    };

    let rx_rpc = {
        let (rx_rpc, task_1, task_2) = rpc_it::builder()
            .with_codec(jsonrpc::Codec)
            .with_frame_writer(tx_server)
            .with_frame_reader(rx_server)
            .with_user_data(server_user_data)
            .with_inbound_queue_capacity(1)
            .with_outbound_queue_capacity(1)
            .build_server(false);

        spawner
            .spawn(async move {
                task_1.await.ok();
            })
            .unwrap();
        spawner
            .spawn(async move {
                task_2.await.ok();
            })
            .unwrap();

        rx_rpc
    };

    (tx_rpc, rx_rpc)
}

#[test]
fn verify_request() {
    let mut executor = LocalPool::new();
    let spawner = executor.spawner();
    let (client, server) = create_default_rpc_pair(&spawner, (), ());

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
                println!("client: sending request");
                let response = client.request(b, "hello", &(1, 2)).await.unwrap();

                println!("server: receiving request");
                let request = server.recv().await.unwrap();

                assert_eq!(request.method(), "hello");
                let (x1, x2) = request.parse::<(i32, i32)>().unwrap();
                assert_eq!((x1, x2), (1, 2));

                println!("server: sending response");
                request.response(b, Ok(x1 + x2)).await.unwrap();

                println!("client: receiving response");
                let response = response.await.unwrap();

                assert_eq!(response.parse::<i32>().unwrap(), 3);

                // Test dropped request
                let response = client.request(b, "hello", &(3, 4)).await.unwrap();
                let request = server.recv().await.unwrap();
                drop(request);

                let err = response.await.unwrap_err();
                let ReceiveResponseError::ErrorResponse(err) = err else {
                    panic!()
                };

                assert_eq!(err.errc(), ResponseError::Unhandled);

                drop(tc);
            })
            .unwrap();
    }

    {
        let client = client.clone();
        let server = server.clone();
        let tc = test_counter.clone();

        spawner
            .spawn(async move {
                // TODO: test notify / request mix
            })
            .unwrap();
    }

    executor.run_until_stalled();

    // Check if all tests were executed/passed
    assert_eq!(Arc::strong_count(&test_counter), 1);
}
