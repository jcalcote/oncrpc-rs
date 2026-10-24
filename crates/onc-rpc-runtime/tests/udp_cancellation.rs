//! Cancellation detaches one UDP waiter without retransmission or peer shutdown.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClient, CallCancellation, CallOptions, Client,
    ClientConfig, MessageBody, OpaqueAuth, Procedure, ProgramVersion, ReplyBody, RpcMessage,
    RuntimeError, TokioAsyncUdpClientTransport, TokioUdpClientTransport,
    decode_rpc_message_datagram, encode_rpc_message_datagram,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{net::UdpSocket, sync::oneshot};

fn program() -> ProgramVersion {
    ProgramVersion {
        program: 42,
        version: 1,
    }
}

fn echo(request: RpcMessage) -> Bytes {
    let MessageBody::Call(call) = request.body else {
        panic!("expected RPC call")
    };
    encode_rpc_message_datagram(&RpcMessage {
        xid: request.xid,
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(call.payload),
        })),
    })
    .unwrap()
}

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("UDP cancellation test exceeded hang guard")
}

#[tokio::test]
async fn cancelled_udp_call_leaves_concurrent_call_and_connection_usable() {
    bounded(async {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(socket.local_addr().unwrap());
        let transport = TokioAsyncUdpClientTransport::connect(&config)
            .await
            .unwrap();
        let client = Arc::new(AsyncClient::new(config, transport));
        let cancellation = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(cancellation.clone());
        let (received, ready) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut buffer = [0; 512];
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
                let request = decode_rpc_message_datagram(&buffer[..length]).unwrap();
                requests.push((request, peer));
            }
            received.send(()).unwrap();
            released.await.unwrap();
            // One response is now late. It must not complete another XID's call.
            for (request, peer) in requests {
                socket.send_to(&echo(request), peer).await.unwrap();
            }
            let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
            let request = decode_rpc_message_datagram(&buffer[..length]).unwrap();
            socket.send_to(&echo(request), peer).await.unwrap();
        });
        let cancelled_client = client.clone();
        let cancelled = tokio::spawn(async move {
            cancelled_client
                .call_typed_with_options::<_, u32>(program(), Procedure(1), &11u32, &options)
                .await
        });
        let healthy_client = client.clone();
        let healthy = tokio::spawn(async move {
            healthy_client
                .call_typed::<_, u32>(program(), Procedure(1), &22u32)
                .await
        });
        ready.await.unwrap();
        cancellation.cancel();
        assert!(matches!(
            cancelled.await.unwrap(),
            Err(RuntimeError::Cancelled)
        ));
        release.send(()).unwrap();
        assert_eq!(healthy.await.unwrap().unwrap(), 22);
        assert_eq!(
            client
                .call_typed::<_, u32>(program(), Procedure(1), &33u32)
                .await
                .unwrap(),
            33
        );
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn precancelled_udp_call_sends_no_request() {
    bounded(async {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(socket.local_addr().unwrap());
        let transport = TokioAsyncUdpClientTransport::connect(&config)
            .await
            .unwrap();
        let client = AsyncClient::new(config, transport);
        let cancellation = CallCancellation::new();
        cancellation.cancel();
        let options = CallOptions::new().with_cancellation(cancellation);
        assert!(matches!(
            client
                .call_typed_with_options::<_, u32>(program(), Procedure(1), &11u32, &options)
                .await,
            Err(RuntimeError::Cancelled)
        ));
        let server = tokio::spawn(async move {
            let mut buffer = [0; 512];
            let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
            let request = decode_rpc_message_datagram(&buffer[..length]).unwrap();
            let MessageBody::Call(call) = &request.body else {
                panic!("expected call")
            };
            assert_eq!(call.payload.as_ref(), &22u32.to_be_bytes());
            socket.send_to(&echo(request), peer).await.unwrap();
        });
        assert_eq!(
            client
                .call_typed::<_, u32>(program(), Procedure(1), &22u32)
                .await
                .unwrap(),
            22
        );
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn separate_clients_share_udp_xids_after_cancellation() {
    bounded(async {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(socket.local_addr().unwrap());
        let transport = TokioAsyncUdpClientTransport::connect(&config)
            .await
            .unwrap();
        let first = AsyncClient::new(config.clone(), transport.clone());
        let second = AsyncClient::new(config, transport);
        let cancellation = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(cancellation.clone());
        let (received, ready) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut bytes = [0; 512];
            let (length, peer) = socket.recv_from(&mut bytes).await.unwrap();
            let first = decode_rpc_message_datagram(&bytes[..length]).unwrap();
            received.send(()).unwrap();
            let (length, second_peer) = socket.recv_from(&mut bytes).await.unwrap();
            let second = decode_rpc_message_datagram(&bytes[..length]).unwrap();
            assert_ne!(
                first.xid, second.xid,
                "clients sharing one physical socket must not reuse XIDs"
            );
            socket.send_to(&echo(first), peer).await.unwrap();
            socket.send_to(&echo(second), second_peer).await.unwrap();
        });
        let first = tokio::spawn(async move {
            first
                .call_typed_with_options::<_, u32>(program(), Procedure(1), &11u32, &options)
                .await
        });
        ready.await.unwrap();
        cancellation.cancel();
        assert!(matches!(first.await.unwrap(), Err(RuntimeError::Cancelled)));
        assert_eq!(
            second
                .call_typed::<_, u32>(program(), Procedure(1), &22u32)
                .await
                .unwrap(),
            22
        );
        server.await.unwrap();
    })
    .await;
}

#[test]
fn synchronous_udp_transport_forwards_cancellation_options() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let config = ClientConfig::new(socket.local_addr().unwrap())
        .with_default_call_timeout(Duration::from_secs(10));
    let cancellation = CallCancellation::new();
    let server_cancellation = cancellation.clone();
    let server = std::thread::spawn(move || {
        let mut buffer = [0; 512];
        let (length, _) = socket.recv_from(&mut buffer).unwrap();
        let request = decode_rpc_message_datagram(&buffer[..length]).unwrap();
        let MessageBody::Call(call) = request.body else {
            panic!("expected call")
        };
        assert_eq!(call.payload.as_ref(), &11u32.to_be_bytes());
        server_cancellation.cancel();
        let (length, peer) = socket.recv_from(&mut buffer).unwrap();
        let request = decode_rpc_message_datagram(&buffer[..length]).unwrap();
        socket.send_to(&echo(request), peer).unwrap();
    });
    let transport = TokioUdpClientTransport::connect(&config).unwrap();
    let client = Client::new(config, transport);
    let options = CallOptions::new().with_cancellation(cancellation);
    assert!(matches!(
        client.call_typed_with_options::<_, u32>(program(), Procedure(1), &11u32, &options),
        Err(RuntimeError::Cancelled)
    ));
    assert_eq!(
        client
            .call_typed::<_, u32>(program(), Procedure(1), &22u32)
            .unwrap(),
        22
    );
    server.join().unwrap();
}
