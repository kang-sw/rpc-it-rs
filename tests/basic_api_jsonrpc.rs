#![cfg(feature = "jsonrpc")]

mod common;

use common::WriteAdapter;
use rpc_it::ext_codec::jsonrpc;

#[test]
fn verify_notify() {
    let (tx, rx) = mpsc::bounded::<bytes::Bytes>(1);
    let tx = WriteAdapter(tx);

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
        let msg = rx.recv().await.unwrap();
        assert_eq!(
            msg,
            &b"{\"jsonrpc\":\"2.0\",\"method\":\"test\",\"params\":{\"test\":\"test\"}}"[..]
        );

        let msg = rx.recv().await.unwrap();
        assert_eq!(
            msg,
            &b"{\"jsonrpc\":\"2.0\",\"method\":\"t-e-st-23\",\"params\":1234}"[..]
        );

        let msg = rx.recv().await.unwrap();
        assert_eq!(
            msg,
            &b"{\"jsonrpc\":\"2.0\",\"method\":\"close\",\"params\":null}"[..]
        );

        rx.recv().await.unwrap_err();
    };

    futures::executor::block_on(async {
        let (r_w_task, ..) = futures::join!(task_runner, task_send, task_recv);

        assert!(matches!(
            r_w_task.unwrap(),
            rpc_it::error::WriteRunnerExitType::ManualClose
        ));
    });
}
