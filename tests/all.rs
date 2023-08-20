use futures_util::join;
use rpc_it::{kv_pairs, rpc::MessageMethodName, Message, RecvMsg};

async fn test_basic_io(server: rpc_it::Transceiver, client: rpc_it::Client) {
    let task_server = async move {
        while let Ok(msg) = server.recv().await {
            let RecvMsg::Request(req) = msg else { unreachable!() };

            let method_name = req.method().expect("method name is invalid utf-8");
            // println!("receving request: {}", method_name);

            if method_name.starts_with("add-") {
                let [a, b] = req.parse::<[i32; 2]>().expect("parse failed");

                assert_eq!(method_name, format!("add-{a}-plus-{b}"));
                req.response(Ok(a + b).as_ref()).await.expect("response failed");
            } else if method_name.starts_with("drop-me") {
                drop(req);
            } else {
            }
        }
    };

    let task_client = async move {
        for i in 0..4000 {
            match i % 2 {
                0 => {
                    // Verify 'Add' operation
                    let a = i * 1000;
                    let b = a + 2000;
                    let method_name = format!("add-{a}-plus-{b}");
                    // println!("sending request: {}", method_name);

                    let req = client.request(&method_name, &(a, b)).await;
                    // println!("req sent");

                    let resp = req.expect("request failed").await;
                    // println!("response received");

                    let value = resp
                        .expect("receiving response failed")
                        .parse::<i32>()
                        .expect("parsing retunred value failed");

                    print!("{} + {} = {} ... \r", a, b, value);
                    assert_eq!(value, a + b);
                }
                1 => {
                    // Verify 'Dropped' error
                    let req = client
                        .request("drop-me", &kv_pairs!("a" = 1, "b" = 3, "c" = 4))
                        .await
                        .unwrap()
                        .await
                        .unwrap();

                    assert!(req.is_error());
                }
                _ => unreachable!(),
            }
        }

        client.close();
    };

    join!(task_server, task_client);
}

#[cfg(all(feature = "in-memory", feature = "jsonrpc"))]
#[tokio::test]
async fn test_basic_io_jsonrpc() {
    let (tx_server, rx_client) = rpc_it::transports::new_in_memory();
    let (tx_client, rx_server) = rpc_it::transports::new_in_memory();

    let (server, task) = rpc_it::Builder::default()
        .with_read(rx_server)
        .with_write(tx_server)
        .with_event_listener(LoggingSubscriber("server"))
        .with_feature(rpc_it::Feature::ENABLE_AUTO_RESPONSE)
        .with_codec(rpc_it::codecs::jsonrpc::Codec::default())
        .build();

    tokio::spawn(task);

    let (client, task) = rpc_it::Builder::default()
        .with_read(rx_client)
        .with_write(tx_client)
        .with_event_listener(LoggingSubscriber("client"))
        .with_codec(rpc_it::codecs::jsonrpc::Codec::default())
        .build();

    tokio::spawn(task);

    test_basic_io(server, client.into_sender()).await;
}

struct LoggingSubscriber(&'static str);
impl rpc_it::InboundEventSubscriber for LoggingSubscriber {
    fn on_close(&self, closed_by_us: bool, result: std::io::Result<()>) {
        println!("[{}] closing: closed-by-us={closed_by_us}, result={result:?}", self.0);
    }

    fn on_inbound_error(&self, error: rpc_it::InboundError) {
        println!("[{}] inbound error: {:?}", self.0, error);
    }
}
