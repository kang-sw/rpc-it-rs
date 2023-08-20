use futures_util::join;
use rpc_it::{rpc::MessageMethodName, Message, RecvMsg};

async fn test_basic_io(server: rpc_it::Channel, client: rpc_it::Client) {
    let task_server = async move {
        while let Ok(msg) = server.recv().await {
            let RecvMsg::Request(req) = msg else { unreachable!() };

            let method_name = req.method().expect("method name is invalid utf-8");
            let [a, b] = req.parse::<[i32; 2]>().expect("parse failed");

            assert_eq!(method_name, format!("add-{a}-plus-{b}"));
            req.response(Ok(a + b).as_ref()).await.expect("response failed");
        }
    };

    let task_client = async move {
        for i in 0..1000 {
            let a = i + 1000;
            let b = a + 2000;
            let method_name = format!("add-{a}-plus-{b}");

            let value = client
                .request(&method_name, &(a, b))
                .await
                .expect("request failed")
                .await
                .expect("receiving response failed")
                .parse::<i32>()
                .expect("parsing retunred value failed");

            assert_eq!(value, a + b);
        }

        client.close();
    };

    join!(task_server, task_client);
}

#[cfg(all(feature = "in-memory", feature = "jsonrpc"))]
#[tokio::test]
async fn test_basic_io_jsonrpc() {
    let (tx_server, rx_server) = rpc_it::transports::new_in_memory();
    let (tx_client, rx_client) = rpc_it::transports::new_in_memory();

    let (server, task) = rpc_it::Builder::default()
        .with_read(rx_server)
        .with_write(tx_server)
        .with_codec(rpc_it::codecs::jsonrpc::Codec::default())
        .build();

    fn is_send<T: Send>(_: T) {}
    is_send(&task);
}
