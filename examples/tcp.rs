use rpc_it::{ext_tokio, Inbound};
use tokio_full as tokio;

#[tokio::main]
async fn main() {
    let svc = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let addr = svc.local_addr().unwrap();

    let make_server = async {
        let (server, _) = svc.accept().await.unwrap();
        let (s_read, s_write) = server.into_split();

        let (server, server_task_1, server_task_2) = rpc_it::InitInfo::builder()
            .write(ext_tokio::WriteAdapter::boxed(s_write))
            .read(ext_tokio::ReadAdapter::boxed(s_read))
            .build()
            .start();

        // Rpc task must be spawned
        tokio::spawn(server_task_1);
        tokio::spawn(server_task_2);

        server
    };

    let make_client = async {
        let client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (c_read, c_write) = client.into_split();

        let (client, client_task_1, client_task_2) = rpc_it::InitInfo::builder()
            .write(ext_tokio::WriteAdapter::boxed(c_write))
            .read(ext_tokio::ReadAdapter::boxed(c_read))
            .build()
            .start();

        // Rpc task must be spawned
        tokio::spawn(client_task_1);
        tokio::spawn(client_task_2);

        client
    };

    // For convenience, we've referred to them as server and client, but in reality both objects
    // are of type `rpc_it::Handle`, which can perform notify, request, and reply functions in
    // both directions.
    let (server, client) = tokio::join!(make_server, make_client);

    let server_task = async {
        // The payload can be split into multiple segments.
        let _ = server
            .notify("route-name-here", [&b"payload"[..], &b"here"[..]])
            .await;

        // Let's send request.
        let rep = server
            .request("discarded-reply", [&b"second"[..], &b"payload"[..]])
            .await
            .unwrap()
            .await
            .unwrap();

        // Oops, the request seems discarded by client.
        assert_eq!(rep.errc(), rpc_it::ReplyCode::Aborted);
        assert_eq!(rep.payload(), b"");

        // Send another request, but this time client may reply.
        let rep_wait = server
            .request("awaited-reply", [&b"third"[..], &b"payload"[..]])
            .await
            .unwrap();

        let req_id = rep_wait.request_id();

        // Unless the connection is broken, reply always returns a value regardless of the
        // reply code.
        let reply = rep_wait.await.unwrap();
        assert_eq!(reply.errc(), rpc_it::ReplyCode::NoRoute);
        assert_eq!(reply.payload(), b"awaited-reply");

        // The request id is the same as the reply id.
        assert_eq!(reply.request_id(), req_id);

        // A handle can be used inbound receiver either.
        //
        // NOTE: In this example, we're doing both sending and receiving with a single handle,
        //       but since handles are cloneable objects, it's recommended to split them into
        //       multiple tasks to distribute commands or separate read/write operations.
        let Inbound::Request(req) = server.recv_inbound().await.unwrap() else { panic!() };
        assert_eq!(req.route(), b"goodbye");
        assert_eq!(req.payload(), b"please reply");

        // You can write a reply.
        req.reply([b"okay, goodbye"]).await.unwrap();

        // Just close the server ..
        server.shutdown().await.unwrap();
    };

    let client_task = async {
        // The logic expected by server_task above
        let Inbound::Notify(msg) = client.recv_inbound().await.unwrap() else { panic!() };
        assert_eq!(msg.route(), b"route-name-here");
        assert_eq!(msg.payload(), b"payloadhere");

        let Inbound::Request(req) = client.recv_inbound().await.unwrap() else { panic!() };
        assert_eq!(req.route(), b"discarded-reply");
        assert_eq!(req.payload(), b"secondpayload");

        // If we discard this request, a 'RepCode::Aborted' will be automatically
        // sent to remote end.
        drop(req);

        let Inbound::Request(req) = client.recv_inbound().await.unwrap() else { panic!() };
        assert_eq!(req.route(), b"awaited-reply");
        assert_eq!(req.payload(), b"thirdpayload");

        // Reply no-route.
        req.error_no_route().await.unwrap();

        // Send a request with reply.
        let rep_wait = client
            .request("goodbye", [&b"please reply"[..]])
            .await
            .unwrap();

        // The reply is expected to be sent by server.
        let rep = rep_wait.await.unwrap();
        assert_eq!(rep.errc(), rpc_it::ReplyCode::Okay);
        assert_eq!(rep.payload(), b"okay, goodbye");
    };

    tokio::join!(server_task, client_task);
}
