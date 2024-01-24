#![cfg(feature = "in-memory-io")]

use futures::task::{Spawn, SpawnExt};
use rpc_it::{io::InMemoryRx, rpc::Config};

#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    pub client_tx_queue_cap: usize,
    pub server_tx_queue_cap: usize,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            client_tx_queue_cap: 1,
            server_tx_queue_cap: 1,
        }
    }
}

pub fn create_default_rpc_pair<R: Config>(
    spawner: &impl Spawn,
    server_user_data: R::UserData,
    client_user_data: R::UserData,
    codec: impl Fn() -> R::Codec,
) -> (rpc_it::RequestSender<R>, rpc_it::Receiver<R, InMemoryRx>) {
    create_rpc_pair(
        spawner,
        server_user_data,
        client_user_data,
        codec(),
        codec(),
        Default::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_rpc_pair<RS: Config, RC: Config>(
    spawner: &impl Spawn,
    server_user_data: RS::UserData,
    client_user_data: RC::UserData,
    server_codec: RS::Codec,
    client_codec: RC::Codec,
    cfg: ConnectionConfig,
) -> (rpc_it::RequestSender<RC>, rpc_it::Receiver<RS, InMemoryRx>) {
    let (tx_client, rx_server) = rpc_it::io::in_memory(1);
    let (tx_server, rx_client) = rpc_it::io::in_memory(1);

    let tx_rpc = {
        let (tx_rpc, task_1, task_2) = rpc_it::builder()
            .with_codec(client_codec)
            .with_frame_writer(tx_client)
            .with_frame_reader(rx_client)
            .with_user_data(client_user_data)
            .with_outbound_queue_capacity(cfg.client_tx_queue_cap)
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
        let (rx_rpc, task_2) = rpc_it::builder()
            .with_codec(server_codec)
            .with_frame_writer(tx_server)
            .with_frame_reader(rx_server)
            .with_user_data(server_user_data)
            .with_outbound_queue_capacity(cfg.server_tx_queue_cap)
            .build_server(false);

        spawner
            .spawn(async move {
                task_2.await.ok();
            })
            .unwrap();

        rx_rpc
    };

    (tx_rpc, rx_rpc)
}
