use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClient, CallRequest, Client, ClientConfig, MessageBody,
    OpaqueAuth, Procedure, ProgramVersion, ReplyBody, RpcMessage, RuntimeError,
    TokioAsyncUdpClientTransport, TokioUdpClientTransport, Xid, decode_rpc_message_datagram,
    encode_rpc_message_datagram,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;

fn reply(xid: Xid, payload: Bytes) -> RpcMessage {
    RpcMessage {
        xid,
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(payload),
        })),
    }
}

fn request_payload(message: &RpcMessage) -> Bytes {
    match &message.body {
        MessageBody::Call(call) => call.payload.clone(),
        MessageBody::Reply(_) => panic!("expected call message"),
    }
}

#[tokio::test]
async fn async_udp_transport_correlates_concurrent_requests_by_xid() {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("socket should bind");
    let addr = socket.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        let mut buffer = vec![0_u8; 4096];

        let (first_len, first_peer) = socket
            .recv_from(&mut buffer)
            .await
            .expect("first recv should succeed");
        let first = decode_rpc_message_datagram(&buffer[..first_len]).expect("first decode");

        let (second_len, second_peer) = socket
            .recv_from(&mut buffer)
            .await
            .expect("second recv should succeed");
        let second = decode_rpc_message_datagram(&buffer[..second_len]).expect("second decode");

        let second_reply =
            encode_rpc_message_datagram(&reply(second.xid, request_payload(&second)))
                .expect("encode should succeed");
        socket
            .send_to(&second_reply, second_peer)
            .await
            .expect("second reply should send");

        let first_reply = encode_rpc_message_datagram(&reply(first.xid, request_payload(&first)))
            .expect("encode should succeed");
        socket
            .send_to(&first_reply, first_peer)
            .await
            .expect("first reply should send");
    });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, transport);

    let first = client.call_typed::<u32, u32>(
        ProgramVersion {
            program: 100_003,
            version: 3,
        },
        Procedure(1),
        &11,
    );
    let second = client.call_typed::<u32, u32>(
        ProgramVersion {
            program: 100_003,
            version: 3,
        },
        Procedure(1),
        &22,
    );

    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.expect("first call should succeed"), 11);
    assert_eq!(second.expect("second call should succeed"), 22);

    server.await.expect("server task should complete");
}

#[tokio::test]
async fn async_udp_transport_retries_until_reply_arrives() {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("socket should bind");
    let addr = socket.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        let mut buffer = vec![0_u8; 4096];

        let (first_len, _) = socket
            .recv_from(&mut buffer)
            .await
            .expect("first recv should succeed");
        let first = decode_rpc_message_datagram(&buffer[..first_len]).expect("first decode");

        let (second_len, second_peer) = socket
            .recv_from(&mut buffer)
            .await
            .expect("second recv should succeed");
        let second = decode_rpc_message_datagram(&buffer[..second_len]).expect("second decode");
        assert_eq!(second.xid, first.xid);

        let encoded = encode_rpc_message_datagram(&reply(second.xid, request_payload(&second)))
            .expect("reply should encode");
        socket
            .send_to(&encoded, second_peer)
            .await
            .expect("reply should send");
    });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_millis(700));
    let transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, transport);

    let response = client
        .call(CallRequest::new(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            Bytes::from_static(b"retry-me"),
        ))
        .await
        .expect("call should succeed after retry");

    assert_eq!(response.payload, Bytes::from_static(b"retry-me"));
    server.await.expect("server task should complete");
}

#[tokio::test]
async fn async_udp_transport_sustains_many_concurrent_calls() {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("socket should bind");
    let addr = socket.local_addr().expect("local addr");
    const CALLS: usize = 24;

    let server = tokio::spawn(async move {
        let mut buffer = vec![0_u8; 4096];
        let mut requests = Vec::with_capacity(CALLS);

        while requests.len() < CALLS {
            let (len, peer) = socket
                .recv_from(&mut buffer)
                .await
                .expect("recv should succeed");
            let request = decode_rpc_message_datagram(&buffer[..len]).expect("decode should work");
            requests.push((peer, request));
        }

        for (peer, request) in requests.into_iter().rev() {
            let encoded =
                encode_rpc_message_datagram(&reply(request.xid, request_payload(&request)))
                    .expect("reply encode should succeed");
            socket
                .send_to(&encoded, peer)
                .await
                .expect("reply should send");
        }
    });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = Arc::new(AsyncClient::new(config, transport));
    let mut tasks = JoinSet::new();

    for value in 0..CALLS as u32 {
        let client = client.clone();
        tasks.spawn(async move {
            client
                .call_typed::<u32, u32>(
                    ProgramVersion {
                        program: 100_003,
                        version: 3,
                    },
                    Procedure(1),
                    &value,
                )
                .await
        });
    }

    let mut results = Vec::with_capacity(CALLS);
    while let Some(result) = tasks.join_next().await {
        results.push(
            result
                .expect("task should join")
                .expect("call should succeed"),
        );
    }
    results.sort_unstable();
    assert_eq!(results, (0..CALLS as u32).collect::<Vec<_>>());

    server.await.expect("server task should complete");
}

#[test]
fn sync_udp_transport_performs_round_trip() {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let server_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        runtime.block_on(async move {
            let socket = UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("socket should bind");
            let addr = socket.local_addr().expect("local addr");
            tx.send(addr).expect("addr send should succeed");

            let mut buffer = vec![0_u8; 4096];
            let (len, peer) = socket
                .recv_from(&mut buffer)
                .await
                .expect("recv should succeed");
            let request = decode_rpc_message_datagram(&buffer[..len]).expect("decode should work");
            let encoded =
                encode_rpc_message_datagram(&reply(request.xid, request_payload(&request)))
                    .expect("reply encode should succeed");
            socket
                .send_to(&encoded, peer)
                .await
                .expect("reply should send");
        });
    });

    let addr = rx.recv().expect("addr recv should succeed");
    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let transport = TokioUdpClientTransport::connect(&config).expect("client should connect");
    let client = Client::new(config, transport);

    let reply: u32 = client
        .call_typed(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &77_u32,
        )
        .expect("call should succeed");

    assert_eq!(reply, 77);
    server_thread.join().expect("server thread should join");
}

#[tokio::test]
async fn async_udp_transport_times_out_when_no_reply_arrives() {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("socket should bind");
    let addr = socket.local_addr().expect("local addr");

    let _server = tokio::spawn(async move {
        let mut buffer = vec![0_u8; 4096];
        let _ = socket
            .recv_from(&mut buffer)
            .await
            .expect("recv should succeed");
        let _ = socket
            .recv_from(&mut buffer)
            .await
            .expect("retry recv should succeed");
    });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_millis(300));
    let transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, transport);

    let error = client
        .call_typed::<u32, u32>(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &5_u32,
        )
        .await
        .expect_err("call should time out");

    assert!(matches!(error, RuntimeError::Transport(message) if message.contains("call timeout")));
}
