#![cfg(feature = "service-macros")]

use std::sync::{atomic::AtomicU32, Arc};

use rpc_it::{codec::Codec, ExactMatchRouter, Sender, ServiceBuilder, Transceiver};

#[rpc_it::service]
trait TestService {
    #[sync]
    #[skip]
    #[no_user_data]
    fn get_int_id(&self) -> u32;

    #[sync]
    #[route = "Add.MyName.IsAdded"]
    #[no_user_data]
    fn add(a: u32, b: u32) -> u32;

    #[sync]
    #[no_user_data]
    fn get_id(&self) -> u32 {
        self.get_int_id()
    }

    #[sync]
    #[no_user_data]
    fn id_added(&self, id: u32) -> u32;

    #[sync]
    #[no_user_data]
    fn update_id(&self, new_id: u32);

    #[no_user_data]
    fn concat_str_with_id(&self, values: (i32, i32, String)) -> String;
}

fn concat(values: &(i32, i32, String)) -> String {
    format!("{}{}{}", values.0, values.1, values.2)
}

async fn execute_service(x: Transceiver) {
    let mut sb = ServiceBuilder::<ExactMatchRouter>::default();

    #[derive(Clone)]
    struct MyNewType(Arc<AtomicU32>);
    let my_state = MyNewType(AtomicU32::default().into());

    impl test_service::Service for MyNewType {
        fn add(a: u32, b: u32) -> u32 {
            a + b
        }

        fn id_added(&self, id: u32) -> u32 {
            id + self.0.fetch_add(id, std::sync::atomic::Ordering::Relaxed)
        }

        fn update_id(&self, new_id: u32) {
            self.0.store(new_id, std::sync::atomic::Ordering::Relaxed);
        }

        fn concat_str_with_id(
            &self,
            values: (i32, i32, String),
            rep: rpc_it::TypedRequest<String, ()>,
        ) {
            rep.ok(&concat(&values)).ok();
        }

        fn get_int_id(&self) -> u32 {
            self.0.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    test_service::load_service(my_state.clone(), &mut sb).unwrap();
    test_service::load_service(my_state.clone(), &mut sb).unwrap_err();

    let service = sb.build();

    while let Ok(x) = x.recv().await {
        service.route_message(x).unwrap();
    }
}

async fn execute_client(x: Sender) {
    let stub = test_service::Client::new(x);
    assert!(stub.add(&1, &2).await.unwrap() == 3);
    assert!(stub.get_id().await.unwrap() == 0);
    assert!(stub.id_added(&1).await.unwrap() == 1);
    assert!(stub.get_id().await.unwrap() == 1);

    stub.update_id(&2).await.unwrap();
    assert!(stub.get_id().await.unwrap() == 2);

    assert!(stub.concat_str_with_id(&(1, 2, "3".into())).await.unwrap() == "123");
}

async fn generated_io_test<T: Codec>(create_codec: impl Fn() -> T) {
    let (tx_server, rx_client) = rpc_it::transports::new_in_memory();
    let (tx_client, rx_server) = rpc_it::transports::new_in_memory();

    let (server, task) = rpc_it::Builder::default()
        .with_read(rx_server)
        .with_write(tx_server)
        .with_feature(rpc_it::Feature::ENABLE_AUTO_RESPONSE)
        .with_codec(create_codec())
        .build();

    tokio::spawn(task);

    let (client, task) = rpc_it::Builder::default()
        .with_read(rx_client)
        .with_request()
        .with_write(tx_client)
        .with_codec(create_codec())
        .build();

    tokio::spawn(task);

    tokio::join!(execute_service(server), execute_client(client.into_sender()));
}

#[cfg(all(feature = "in-memory", feature = "msgpack-rpc"))]
#[tokio::test]
async fn test_generated_io_msgpack_rpc() {
    generated_io_test(|| {
        rpc_it::codecs::msgpack_rpc::Codec::default()
            .with_auto_wrapping(true)
            .with_unwrap_mono_param(true)
    })
    .await;
}
