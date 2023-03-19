use futures::AsyncReadExt;
use rpc_it::{
    ext_futures,
    ext_tokio::{ReadAdapter, WriteAdapter},
    Inbound, RetrievePayload, RetrieveRoute,
};
use tokio::join;
use tokio_full as tokio;

/* ---------------------------------------------------------------------------------------------- */
/*                                           TEST CASES                                           */
/* ---------------------------------------------------------------------------------------------- */

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
    assert_eq!(reply.errc(), rpc_it::ReplyCode::Okay);

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
    assert_eq!(reply.errc(), rpc_it::ReplyCode::Aborted);

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

async fn test_basic_cases_sync(server: rpc_it::Handle, client: rpc_it::Handle) {
    /* --------------------------------------- C->S Notify -------------------------------------- */
    client
        .post_notify(
            "this is first route",
            Some(client.pool().checkout_copied(&[b"hello, world!"])),
        )
        .expect("no fail");

    let Inbound::Notify(noti) = server.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(noti.route(), b"this is first route");
    assert_eq!(noti.payload(), b"hello, world!");

    /* ----------------------------- S->C Request, Reply, ReplyRecv ----------------------------- */
    let reply = server
        .post_request(
            "this request",
            Some(
                client
                    .pool()
                    .checkout_copied(&[&b"second,"[..], &b" world!"[..]]),
            ),
        )
        .unwrap();

    let id = reply.request_id();

    let Inbound::Request(req) = client.recv_inbound().await.unwrap() else { panic!() };
    assert_eq!(req.route(), b"this request");
    assert_eq!(req.payload(), b"second, world!");

    req.post_reply(client.pool().checkout_copied(&[&b"i replied!"[..]]).into())
        .unwrap();

    let reply = reply.await.unwrap();
    assert_eq!(reply.payload(), b"i replied!");
    assert_eq!(reply.request_id(), id);
    assert_eq!(reply.errc(), rpc_it::ReplyCode::Okay);

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
    assert_eq!(reply.errc(), rpc_it::ReplyCode::Aborted);

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

/* ---------------------------------------------------------------------------------------------- */
/*                                           BENCHMARKS                                           */
/* ---------------------------------------------------------------------------------------------- */

async fn _benchmark(_server: rpc_it::Handle, _client: rpc_it::Handle, _case: usize) {
    todo!()
}

/* ---------------------------------------------------------------------------------------------- */
/*                                         TRANSPORT LAYER                                        */
/* ---------------------------------------------------------------------------------------------- */

#[tokio::test]
async fn tokio_basic_cases() {
    let svc = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let addr = svc.local_addr().unwrap();

    let accept_task = async { svc.accept().await.unwrap() };
    let connect_task = async { tokio::net::TcpStream::connect(addr).await.unwrap() };

    let (client, server) = tokio::join!(connect_task, accept_task);
    let (c_read, c_write) = client.into_split();
    let (s_read, s_write) = server.0.into_split();

    let (client, ct1, ct2) = rpc_it::InitInfo::builder()
        .write(WriteAdapter::boxed(c_write))
        .read(ReadAdapter::boxed(c_read))
        .build()
        .start();

    let (server, st1, st2) = rpc_it::InitInfo::builder()
        .write(WriteAdapter::boxed(s_write))
        .read(ReadAdapter::boxed(s_read))
        .build()
        .start();

    let tasks = tokio::spawn(async move { join!(ct1, ct2, st1, st2) });

    /* ---------------------------------------- Test It! ---------------------------------------- */
    test_basic_cases(server.clone(), client.clone()).await;
    test_basic_cases_sync(server.clone(), client.clone()).await;

    // Assure all tasks drop after dropping all handles.
    drop((server, client));
    let _ = tasks.await;
}

#[tokio::test]
async fn inmemory_basic_cases() {
    let (s, c) = futures_ringbuf::Endpoint::pair(1924, 2440);
    let ((s_r, s_w), (c_r, c_w)) = (s.split(), c.split());

    let (client, ct1, ct2) = rpc_it::InitInfo::builder()
        .write(ext_futures::WriteAdapter::boxed(s_w))
        .read(ext_futures::ReadAdapter::boxed(s_r))
        .build()
        .start();

    let (server, st1, st2) = rpc_it::InitInfo::builder()
        .write(ext_futures::WriteAdapter::boxed(c_w))
        .read(ext_futures::ReadAdapter::boxed(c_r))
        .build()
        .start();

    let tasks = tokio::spawn(async move { join!(ct1, ct2, st1, st2) });

    /* ---------------------------------------- Test It! ---------------------------------------- */
    test_basic_cases(server.clone(), client.clone()).await;
    test_basic_cases_sync(server.clone(), client.clone()).await;

    // Won't be closed automatically.
    server.shutdown().await.ok();

    drop((server, client));
    let _ = tasks.await;
}
