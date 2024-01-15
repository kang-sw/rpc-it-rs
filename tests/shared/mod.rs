#![cfg(feature = "in-memory-io")]

use futures::task::{Spawn, SpawnExt};
use rpc_it::{Codec, ReceiveErrorHandler};

#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    pub client_tx_queue_cap: usize,
    pub client_rx_queue_cap: usize,
    pub server_tx_queue_cap: usize,
    pub server_rx_queue_cap: usize,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            client_tx_queue_cap: 1,
            client_rx_queue_cap: 1,
            server_tx_queue_cap: 1,
            server_rx_queue_cap: 1,
        }
    }
}

pub fn create_default_rpc_pair<US, UC, C>(
    spawner: &impl Spawn,
    server_user_data: US,
    client_user_data: UC,
    codec: impl Fn() -> C,
) -> (rpc_it::RequestSender<UC>, rpc_it::Receiver<US>)
where
    US: rpc_it::UserData,
    UC: rpc_it::UserData,
    C: Codec,
{
    create_rpc_pair(
        spawner,
        server_user_data,
        client_user_data,
        (),
        (),
        codec,
        Default::default(),
    )
}

pub fn create_rpc_pair<US, UC, C>(
    spawner: &impl Spawn,
    server_user_data: US,
    client_user_data: UC,
    server_rh: impl ReceiveErrorHandler<US>,
    client_rh: impl ReceiveErrorHandler<UC>,
    codec: impl Fn() -> C,
    cfg: ConnectionConfig,
) -> (rpc_it::RequestSender<UC>, rpc_it::Receiver<US>)
where
    US: rpc_it::UserData,
    UC: rpc_it::UserData,
    C: Codec,
{
    let (tx_client, rx_server) = rpc_it::io::in_memory(1);
    let (tx_server, rx_client) = rpc_it::io::in_memory(1);

    let tx_rpc = {
        let (tx_rpc, task_1, task_2) = rpc_it::builder()
            .with_codec(codec())
            .with_frame_writer(tx_client)
            .with_frame_reader(rx_client)
            .with_user_data(client_user_data)
            .with_inbound_queue_capacity(cfg.client_rx_queue_cap)
            .with_outbound_queue_capacity(cfg.client_tx_queue_cap)
            .with_read_event_handler(client_rh)
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
            .with_codec(codec())
            .with_frame_writer(tx_server)
            .with_frame_reader(rx_server)
            .with_user_data(server_user_data)
            .with_inbound_queue_capacity(cfg.server_rx_queue_cap)
            .with_outbound_queue_capacity(cfg.server_tx_queue_cap)
            .with_read_event_handler(server_rh)
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
