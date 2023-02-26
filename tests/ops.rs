use futures::future::join_all;
use rpc_it::{
    ext::ext_tokio::{ReadAdapter, WriteAdapter},
    Inbound,
};
use tokio_full as tokio;

async fn test_basic_cases(server: rpc_it::Handle, client: rpc_it::Handle) {
    /* --------------------------------------- C->S Notify -------------------------------------- */
    client
        .notify("this is first route", [b"hello, world!"])
        .await
        .expect("no fail");

    let Inbound::Notify(noti) = server.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(noti.route(), b"this is first route");
    assert_eq!(noti.payload(), b"hello, world!");

    /* ----------------------------- S->C Request, Reply, ReplyRecv ----------------------------- */
    let reply = server
        .request("this request", [&b"second,"[..], &b" world!"[..]])
        .await
        .unwrap();

    let id = reply.request_id();

    let Inbound::Request(req) = client.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(req.route(), b"this request");
    assert_eq!(req.payload(), b"second, world!");

    req.reply([b"i replied!"]).await.unwrap();

    let reply = reply.await.unwrap();
    assert_eq!(reply.payload(), b"i replied!");
    assert_eq!(reply.request_id(), id);
    assert_eq!(reply.errc(), rpc_it::RepCode::Okay);

    /* ---------------------------- S->C Request, Discard, ReplyRecv ---------------------------- */
    let reply = server
        .request("second route", [&b"third,"[..], &b" is word!"[..]])
        .await
        .unwrap();

    let id = reply.request_id();
    let Inbound::Request(req) = client.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(req.route(), b"second route");
    assert_eq!(req.payload(), b"third, is word!");

    // Drop the request, so the reply will be discarded.
    assert_eq!(id, req.request_id());
    drop(req);

    // The reply should be discarded.
    let reply = reply.await.unwrap();
    assert_eq!(reply.payload(), b"");
    assert_eq!(reply.request_id(), id);
    assert_eq!(reply.errc(), rpc_it::RepCode::Aborted);

    /* --------------------------- S->c Request, Reply, Discard Reply --------------------------- */
    let reply = server
        .request("third route", [&b"fourth,"[..], &b" is word!"[..]])
        .await
        .unwrap();

    let id = reply.request_id();
    assert_eq!(server.pending_requests(), 1);
    drop(reply);
    assert_eq!(server.pending_requests(), 0);

    let Inbound::Request(req) = client.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(req.route(), b"third route");
    assert_eq!(req.request_id(), id);
    assert_eq!(req.payload(), b"fourth, is word!");

    req.reply([b"i replied!"]).await.unwrap();
    assert_eq!(server.pending_requests(), 0);
}

#[tokio::test]
async fn tokio_basic_cases() {
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

    /* ---------------------------------------- Test It! ---------------------------------------- */
    test_basic_cases(server.clone(), client.clone()).await;

    // Assure all tasks drop after dropping all handles.
    drop((server, client));
    let _ = tasks.await;
}
