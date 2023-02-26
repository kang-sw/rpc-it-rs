use futures::future::join_all;
use rpc_it::{
    ext::ext_tokio::{ReadAdapter, WriteAdapter},
    Inbound,
};
use tokio_full as tokio;

#[tokio::test]
async fn tokio_init() {
    let svc = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let addr = svc.local_addr().unwrap();

    let accept_task = async { svc.accept().await.unwrap() };
    let connect_task = async { tokio::net::TcpStream::connect(addr).await.unwrap() };

    let (client, server) = tokio::join!(connect_task, accept_task);
    let (c_read, c_write) = client.into_split();
    let (s_read, s_write) = server.0.into_split();

    let (client, client_task) = rpc_it::InitInfo::builder()
        .write(WriteAdapter::boxed(c_write))
        .read(ReadAdapter::boxed(c_read))
        .build()
        .start();

    let (server, server_task) = rpc_it::InitInfo::builder()
        .write(WriteAdapter::boxed(s_write))
        .read(ReadAdapter::boxed(s_read))
        .build()
        .start();

    let tasks = tokio::spawn(join_all([client_task, server_task]));

    client
        .notify("this is first route", [b"hello, world!"])
        .await
        .expect("no fail");

    let Inbound::Notify(noti) = server.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(noti.route(), b"this is first route");
    assert_eq!(noti.payload(), b"hello, world!");

    // Assure all tasks drop after dropping all handles.
    drop((server, client));
    let _ = tasks.await;
}
