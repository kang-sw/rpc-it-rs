#![cfg(all(feature = "jsonrpc", feature = "in-memory-io"))]

use futures::StreamExt;
use rpc_it::ext_codec::jsonrpc;

#[test]
fn verify_notify() {
    let (tx, rx) = rpc_it::io::in_memory(128);

    let (tx_rpc, task_runner) = rpc_it::create_builder()
        .with_codec(jsonrpc::Codec)
        .with_frame_writer(tx)
        .with_write_channel_capacity(128)
        .build_write_only();

    let task_send = async {
        let mut buf = bytes::BytesMut::new();
        let b = &mut buf;

        tx_rpc
            .notify(b, "test", &serde_json::json!({ "test": "test" }))
            .await
            .unwrap();

        tx_rpc.notify(b, "t-e-st-23", &1234).await.unwrap();
        tx_rpc.notify(b, "close", &()).await.unwrap();

        tx_rpc.shutdown_writer(false).await.unwrap();
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
