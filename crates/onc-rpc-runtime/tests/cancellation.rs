//! Per-call cancellation preserves shared connections and caller-owned storage.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClientTransport, CallCancellation, CallOptions,
    ClientConfig, ClientTransport, MessageBody, OpaqueAuth, OwnedRpcTransport, Procedure,
    ProgramVersion, ReplyBody, ReplyReceiver, RpcMessage, RuntimeError, SegmentedRequest, Xid,
    read_rpc_message, write_rpc_message,
};
use std::{future::Future, pin::Pin, task::Poll, time::Duration};
use tokio::{
    io::{AsyncWriteExt, duplex},
    sync::mpsc,
};

fn request(xid: u32) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Call(onc_rpc_wire::CallBody::new(
            ProgramVersion {
                program: 0x3123_0001,
                version: 1,
            },
            Procedure(1),
            OpaqueAuth::none(),
            OpaqueAuth::none(),
            Bytes::new(),
        )),
    }
}
fn reply(xid: u32, data: &[u8]) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(Bytes::copy_from_slice(data)),
        })),
    }
}
fn segmented(xid: u32) -> SegmentedRequest {
    SegmentedRequest {
        header: request(xid),
        payload: vec![],
    }
}
fn config() -> ClientConfig {
    ClientConfig::new("127.0.0.1:0".parse().unwrap())
}
async fn held<F: Future>(mut future: Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}
async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("cancellation cleanup must finish")
}
struct Destination {
    bytes: Vec<u8>,
    offset: usize,
    advanced: mpsc::UnboundedSender<usize>,
}
impl ReplyReceiver for Destination {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        Ok(&mut self.bytes[self.offset..])
    }
    fn advance(&mut self, count: usize) -> Result<(), RuntimeError> {
        self.offset += count;
        let _ = self.advanced.send(self.offset);
        Ok(())
    }
}

#[tokio::test]
async fn cancelled_receive_returns_owner_before_late_body_without_touching_other_calls() {
    bounded(async {
        let (stream, mut peer) = duplex(4096);
        let transport = OwnedRpcTransport::from_stream(stream, &config()).unwrap();
        let cancellation = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(cancellation.clone());
        let (advanced, mut progress) = mpsc::unbounded_channel();
        let destination = Destination {
            bytes: vec![0; 8],
            offset: 0,
            advanced,
        };
        let pointer = destination.bytes.as_ptr() as usize;
        let mut first = Box::pin(transport.call_into(segmented(1), destination, &options));
        held(first.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(1));
        let encoded = reply(1, b"abcdefgh").encode().unwrap();
        peer.write_all(&(0x8000_0000 | encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&encoded[..encoded.len() - 6]).await.unwrap();
        assert_eq!(progress.recv().await, Some(2));
        cancellation.cancel();
        let mut outcome = first.await;
        assert_eq!(outcome.result, Err(RuntimeError::Cancelled));
        assert_eq!(outcome.receiver.bytes.as_ptr() as usize, pointer);
        assert_eq!(outcome.payload_len, 2);
        assert_eq!(&outcome.receiver.bytes[..2], b"ab");
        outcome.receiver.bytes.fill(0x5a);
        assert!(!transport.is_closed());
        assert_eq!(transport.admission_snapshot().calls, 0);
        let mut second = Box::pin(transport.call(request(2)));
        held(second.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(2));
        peer.write_all(b"cdefgh").await.unwrap();
        write_rpc_message(&mut peer, &reply(2, b"ok"))
            .await
            .unwrap();
        assert_eq!(second.await.unwrap(), reply(2, b"ok"));
        assert_eq!(outcome.receiver.bytes, vec![0x5a; 8]);
        assert!(!transport.is_closed());
    })
    .await;
}

#[tokio::test]
async fn ordinary_call_can_cancel_during_incomplete_envelope_and_drain_later() {
    bounded(async {
        let (stream, mut peer) = duplex(4096);
        let transport = OwnedRpcTransport::from_stream(stream, &config()).unwrap();
        let token = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(token.clone());
        let mut first = Box::pin(transport.call_with_options(request(11), &options));
        held(first.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        let encoded = reply(11, b"late").encode().unwrap();
        peer.write_all(&(0x8000_0000 | encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&encoded[..7]).await.unwrap();
        token.cancel();
        assert_eq!(first.await, Err(RuntimeError::Cancelled));
        assert!(!transport.is_closed());
        let mut second = Box::pin(transport.call(request(12)));
        held(second.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        peer.write_all(&encoded[7..]).await.unwrap();
        write_rpc_message(&mut peer, &reply(12, b"next"))
            .await
            .unwrap();
        assert_eq!(second.await.unwrap(), reply(12, b"next"));
    })
    .await;
}

#[tokio::test]
async fn explicit_cancellation_finishes_partial_outgoing_frame() {
    bounded(async {
        let (stream, mut peer) = duplex(1);
        let transport = OwnedRpcTransport::from_stream(stream, &config()).unwrap();
        let token = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(token.clone());
        let mut call = Box::pin(transport.call_with_options(request(21), &options));
        held(call.as_mut()).await;
        token.cancel();
        held(call.as_mut()).await;
        assert!(!transport.is_closed());
        let peer_task = tokio::spawn(async move {
            assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(21));
            assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(22));
            write_rpc_message(&mut peer, &reply(22, b"ok"))
                .await
                .unwrap();
        });
        assert_eq!(call.await, Err(RuntimeError::Cancelled));
        assert!(!transport.is_closed());
        assert_eq!(transport.call(request(22)).await.unwrap(), reply(22, b"ok"));
        peer_task.await.unwrap();
    })
    .await;
}

struct Custom;
impl ClientTransport for Custom {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        Ok(reply(request.xid.0, b""))
    }
}
#[onc_rpc_runtime::async_trait]
impl AsyncClientTransport for Custom {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        ClientTransport::call(self, request)
    }
}
#[tokio::test]
async fn custom_transport_defaults_reject_unsupported_cancellation_before_call() {
    let token = CallCancellation::new();
    let options = CallOptions::new().with_cancellation(token.clone());
    assert_eq!(
        ClientTransport::call_with_options(&Custom, request(1), &options),
        Err(RuntimeError::UnsupportedCancellation)
    );
    assert_eq!(
        AsyncClientTransport::call_with_options(&Custom, request(1), &options).await,
        Err(RuntimeError::UnsupportedCancellation)
    );
    token.cancel();
    assert_eq!(
        ClientTransport::call_with_options(&Custom, request(2), &options),
        Err(RuntimeError::Cancelled)
    );
    assert_eq!(
        AsyncClientTransport::call_with_options(&Custom, request(2), &options).await,
        Err(RuntimeError::Cancelled)
    );
}

#[tokio::test]
async fn cancellation_token_is_sticky_shared_and_safe_with_multiple_waiters() {
    let token = CallCancellation::new();
    let other = CallCancellation::new();
    assert_ne!(token, other);
    let copy = token.clone();
    assert_eq!(token, copy);
    let mut first = Box::pin(token.cancelled());
    let mut second = Box::pin(copy.cancelled());
    held(first.as_mut()).await;
    held(second.as_mut()).await;
    token.cancel();
    token.cancel();
    bounded(first).await;
    bounded(second).await;
    assert!(copy.is_cancelled());
    assert!(!other.is_cancelled());
    assert_eq!(token, copy);
    bounded(copy.cancelled()).await;
}

#[tokio::test]
async fn separate_clients_and_raw_calls_share_one_xid_namespace() {
    use onc_rpc_runtime::{AsyncClient, CallRequest};
    bounded(async {
        let (stream, mut peer) = duplex(4096);
        let transport = OwnedRpcTransport::from_stream(stream, &config()).unwrap();
        let first_client = AsyncClient::new(config(), transport.clone());
        let cancellation = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(cancellation.clone());
        let args = || {
            CallRequest::new(
                ProgramVersion {
                    program: 1,
                    version: 1,
                },
                Procedure(1),
                Bytes::new(),
            )
        };
        let mut first = Box::pin(first_client.call_with_options(args(), &options));
        held(first.as_mut()).await;
        let first_xid = read_rpc_message(&mut peer).await.unwrap().xid;
        cancellation.cancel();
        assert_eq!(first.await, Err(RuntimeError::Cancelled));
        // Creating another client over the same transport must not restart XIDs.
        let second_client = AsyncClient::new(config(), transport.clone());
        let mut second = Box::pin(second_client.call(args()));
        held(second.as_mut()).await;
        let second_xid = read_rpc_message(&mut peer).await.unwrap().xid;
        assert_ne!(first_xid, second_xid);
        write_rpc_message(&mut peer, &reply(first_xid.0, b"wrong late result"))
            .await
            .unwrap();
        write_rpc_message(&mut peer, &reply(second_xid.0, b"right result"))
            .await
            .unwrap();
        assert_eq!(second.await.unwrap().payload.as_ref(), b"right result");
        let raw_xid = transport.xid_allocator().allocate().unwrap();
        assert_ne!(raw_xid, first_xid);
        assert_ne!(raw_xid, second_xid);
        let mut raw = Box::pin(transport.call(request(raw_xid.0)));
        held(raw.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, raw_xid);
        write_rpc_message(&mut peer, &reply(raw_xid.0, b"raw"))
            .await
            .unwrap();
        assert_eq!(raw.await.unwrap(), reply(raw_xid.0, b"raw"));
    })
    .await;
}

#[tokio::test]
async fn unreachable_call_and_write_deadlines_do_not_overflow() {
    bounded(async {
        let (stream, mut peer) = duplex(4096);
        let transport =
            OwnedRpcTransport::from_stream(stream, &config().with_write_timeout(Duration::MAX))
                .unwrap();
        let options = CallOptions::new().with_timeout(Duration::MAX);
        let peer_task = tokio::spawn(async move {
            let request = read_rpc_message(&mut peer).await.unwrap();
            write_rpc_message(&mut peer, &reply(request.xid.0, b"ok"))
                .await
                .unwrap();
        });
        assert_eq!(
            transport
                .call_with_options(request(31), &options)
                .await
                .unwrap(),
            reply(31, b"ok")
        );
        peer_task.await.unwrap();
    })
    .await;
}
