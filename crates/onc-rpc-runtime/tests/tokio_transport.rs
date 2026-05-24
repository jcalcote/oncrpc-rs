use bytes::{Bytes, BytesMut};
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClient, CallRequest, Client, ClientConfig, MessageBody,
    OpaqueAuth, Procedure, ProgramVersion, ReplyBody, RpcMessage, TokioAsyncClientTransport,
    TokioClientTransport, Xid, try_decode_message_from_buffer, write_rpc_message,
};
use onc_rpc_wire::fragment_record;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn next_message(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    buffer: &mut BytesMut,
) -> RpcMessage {
    loop {
        if let Some(message) =
            try_decode_message_from_buffer(buffer).expect("buffer should decode cleanly")
        {
            return message;
        }

        let read = reader
            .read_buf(buffer)
            .await
            .expect("socket read should succeed");
        assert!(read > 0, "connection closed before message arrived");
    }
}

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
async fn async_transport_reassembles_fragmented_reply() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept should succeed");
        let (mut reader, mut writer) = stream.into_split();
        let mut buffer = BytesMut::with_capacity(1024);
        let request = next_message(&mut reader, &mut buffer).await;
        let payload = u32::to_be_bytes(777);
        let reply = reply(request.xid, Bytes::copy_from_slice(&payload));
        let encoded = reply.encode().expect("reply should encode");
        for fragment in fragment_record(&encoded, 3).expect("fragmentation should succeed") {
            writer
                .write_all(&fragment)
                .await
                .expect("fragment write should succeed");
        }
    });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let transport = TokioAsyncClientTransport::connect(&config)
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
            Bytes::from_static(b"hello"),
        ))
        .await
        .expect("call should succeed");

    assert_eq!(
        response.payload,
        Bytes::copy_from_slice(&u32::to_be_bytes(777))
    );
    server.await.expect("server task should complete");
}

#[tokio::test]
async fn async_transport_correlates_concurrent_requests_by_xid() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept should succeed");
        let (mut reader, mut writer) = stream.into_split();
        let mut buffer = BytesMut::with_capacity(1024);

        let first = next_message(&mut reader, &mut buffer).await;
        let second = next_message(&mut reader, &mut buffer).await;

        write_rpc_message(
            &mut writer,
            &reply(second.xid, Bytes::copy_from_slice(&u32::to_be_bytes(2))),
        )
        .await
        .expect("second reply should write");
        write_rpc_message(
            &mut writer,
            &reply(first.xid, Bytes::copy_from_slice(&u32::to_be_bytes(1))),
        )
        .await
        .expect("first reply should write");
    });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let transport = TokioAsyncClientTransport::connect(&config)
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
    assert_eq!(first.expect("first call should succeed"), 1);
    assert_eq!(second.expect("second call should succeed"), 2);

    server.await.expect("server task should complete");
}

#[tokio::test]
async fn async_transport_honors_default_call_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr");

    let _server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.expect("accept should succeed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_millis(50));
    let transport = TokioAsyncClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, transport);

    let error = client
        .call(CallRequest::new(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            Bytes::from_static(b"hello"),
        ))
        .await
        .expect_err("call should time out");

    match error {
        onc_rpc_runtime::RuntimeError::Transport(message) => {
            assert!(
                message.contains("call timeout"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected call-timeout transport error, got {other:?}"),
    }
}

#[test]
fn sync_transport_performs_round_trip_over_tokio_io() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let addr = runtime.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept should succeed");
            let (mut reader, mut writer) = stream.into_split();
            let mut buffer = BytesMut::with_capacity(1024);
            let request = next_message(&mut reader, &mut buffer).await;
            write_rpc_message(
                &mut writer,
                &reply(request.xid, Bytes::copy_from_slice(&u32::to_be_bytes(55))),
            )
            .await
            .expect("reply should write");
        });
        addr
    });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let transport = TokioClientTransport::connect(&config).expect("client should connect");
    let client = Client::new(config, transport);

    let response: u32 = client
        .call_typed(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &5_u32,
        )
        .expect("call should succeed");

    assert_eq!(response, 55);
}

#[test]
fn sync_transport_supports_concurrent_calls_on_one_client() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let addr = runtime.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept should succeed");
            let (mut reader, mut writer) = stream.into_split();
            let mut buffer = BytesMut::with_capacity(1024);

            let first = next_message(&mut reader, &mut buffer).await;
            let second = next_message(&mut reader, &mut buffer).await;

            write_rpc_message(&mut writer, &reply(second.xid, request_payload(&second)))
                .await
                .expect("second reply should write");
            write_rpc_message(&mut writer, &reply(first.xid, request_payload(&first)))
                .await
                .expect("first reply should write");
        });
        addr
    });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let transport = TokioClientTransport::connect(&config).expect("client should connect");
    let client = Arc::new(Client::new(config, transport));
    let barrier = Arc::new(Barrier::new(3));

    let first_client = client.clone();
    let first_barrier = barrier.clone();
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_client.call_typed::<u32, u32>(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &11,
        )
    });

    let second_client = client.clone();
    let second_barrier = barrier.clone();
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_client.call_typed::<u32, u32>(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &22,
        )
    });

    barrier.wait();

    assert_eq!(
        first
            .join()
            .expect("first thread should join")
            .expect("first call should succeed"),
        11
    );
    assert_eq!(
        second
            .join()
            .expect("second thread should join")
            .expect("second call should succeed"),
        22
    );
}
