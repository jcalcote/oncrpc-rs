//! Admission happens before an accepted future can write or register an XID.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AdmissionError, AdmissionLimits, AsyncClientTransport,
    CallOptions, CallRefusal, ClientConfig, MessageBody, OpaqueAuth, OwnedRpcTransport, Procedure,
    ProgramVersion, RecordLimits, ReplyBody, ReplyReceiver, RpcMessage, RuntimeError,
    SegmentedRequest, Xid, read_rpc_message, write_rpc_message,
};
use onc_rpc_wire::CallBody;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncReadExt, DuplexStream, duplex};
use tokio::sync::mpsc;

#[derive(Debug)]
struct Destination {
    bytes: Box<[u8]>,
    used: usize,
    drops: Option<mpsc::UnboundedSender<()>>,
}
impl Destination {
    fn new() -> Self {
        Self {
            bytes: vec![0xaa; 8].into_boxed_slice(),
            used: 0,
            drops: None,
        }
    }
    fn pointer(&self) -> usize {
        self.bytes.as_ptr() as usize
    }
}
impl ReplyReceiver for Destination {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        Ok(&mut self.bytes[self.used..])
    }
    fn advance(&mut self, count: usize) -> Result<(), RuntimeError> {
        self.used += count;
        Ok(())
    }
}
impl Drop for Destination {
    fn drop(&mut self) {
        if let Some(drops) = &self.drops {
            let _ = drops.send(());
        }
    }
}
fn request(xid: u32) -> SegmentedRequest {
    SegmentedRequest {
        header: RpcMessage {
            xid: Xid(xid),
            body: MessageBody::Call(CallBody::new(
                ProgramVersion {
                    program: 100003,
                    version: 3,
                },
                Procedure(1),
                OpaqueAuth::none(),
                OpaqueAuth::none(),
                Bytes::new(),
            )),
        },
        payload: vec![Bytes::from_static(b"payload!")],
    }
}
fn config() -> ClientConfig {
    ClientConfig::new("127.0.0.1:0".parse().unwrap())
}
fn limited(
    capacity: usize,
    max_calls: usize,
    max_bytes: usize,
) -> (OwnedRpcTransport, DuplexStream) {
    let (stream, peer) = duplex(capacity);
    let transport = OwnedRpcTransport::from_stream_with_admission(
        stream,
        &config(),
        RecordLimits::default(),
        AdmissionLimits {
            max_calls,
            max_bytes,
        },
    )
    .unwrap();
    (transport, peer)
}
fn accepted<T, E>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(_) => panic!("call should be admitted"),
    }
}
fn refused<T, Q, R>(result: Result<T, CallRefusal<Q, R>>) -> CallRefusal<Q, R> {
    match result {
        Err(value) => value,
        Ok(_) => panic!("call should be refused"),
    }
}
async fn held<F: Future>(future: Pin<&mut F>) {
    let mut future = future;
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}
async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("admission test exceeded hang bound")
}
fn charge() -> usize {
    let (transport, _) = limited(128, 4, usize::MAX);
    let operation =
        accepted(transport.try_call_into(request(1), Destination::new(), &CallOptions::default()));
    let bytes = transport.admission_snapshot().bytes;
    assert!(bytes > 0);
    drop(operation);
    assert_eq!(transport.admission_snapshot().bytes, 0);
    bytes
}
fn reply(xid: Xid) -> RpcMessage {
    RpcMessage {
        xid,
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(Bytes::from_static(b"result!!")),
        })),
    }
}

#[tokio::test]
async fn unpolled_call_reserves_immediately_and_drop_releases_without_io() {
    bounded(async {
        let (transport, mut peer) = limited(128, 1, usize::MAX);
        let operation = accepted(transport.try_call_into(
            request(1),
            Destination::new(),
            &CallOptions::default(),
        ));
        assert_eq!(transport.admission_snapshot().calls, 1);
        assert!(transport.admission_snapshot().bytes > 0);
        let refusal = refused(transport.try_call_into(
            request(2),
            Destination::new(),
            &CallOptions::default(),
        ));
        assert!(matches!(refusal.reason, AdmissionError::AtCapacity));
        assert!(refusal.reason.is_retryable());
        drop(operation);
        assert_eq!(transport.admission_snapshot().calls, 0);
        assert_eq!(transport.admission_snapshot().bytes, 0);
        drop(transport);
        let mut wire = Vec::new();
        peer.read_to_end(&mut wire).await.unwrap();
        assert!(wire.is_empty());
    })
    .await
}

#[tokio::test]
async fn byte_capacity_and_permanent_oversize_are_distinct() {
    bounded(async {
    let required=charge();let (transport,_)=limited(128,100,required*2);
    let first=accepted(transport.try_call_into(request(1),Destination::new(),&CallOptions::default()));
    let second=accepted(transport.try_call_into(request(2),Destination::new(),&CallOptions::default()));
    assert_eq!(transport.admission_snapshot().calls,2);assert_eq!(transport.admission_snapshot().bytes,required*2);
    let refusal=refused(transport.try_call_into(request(3),Destination::new(),&CallOptions::default()));
    assert!(matches!(refusal.reason,AdmissionError::AtCapacity));assert_eq!(refusal.required_bytes,required);
    let mut large=request(4);large.payload=vec![Bytes::from(vec![0;required*4])];
    let refusal=refused(transport.try_call_into(large,Destination::new(),&CallOptions::default()));
    assert!(matches!(refusal.reason,AdmissionError::TooLarge {required:_,limit} if limit==required*2));assert!(!refusal.reason.is_retryable());
    assert!(refusal.required_bytes>required*2);drop(first);drop(second);assert_eq!(transport.admission_snapshot().calls,0);
}).await
}

struct PayloadOwner {
    bytes: Box<[u8]>,
    drops: mpsc::UnboundedSender<()>,
}
impl AsRef<[u8]> for PayloadOwner {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for PayloadOwner {
    fn drop(&mut self) {
        let _ = self.drops.send(());
    }
}

#[tokio::test]
async fn refusal_returns_exact_request_and_receiver_for_retry() {
    bounded(async {
        let (transport, mut peer) = limited(128, 1, usize::MAX);
        let first = accepted(transport.try_call_into(
            request(1),
            Destination::new(),
            &CallOptions::default(),
        ));
        let (send_drops, mut payload_dropped) = mpsc::unbounded_channel();
        let mut proposed = request(2);
        proposed.payload = vec![Bytes::from_owner(PayloadOwner {
            bytes: b"payload!".to_vec().into_boxed_slice(),
            drops: send_drops,
        })];
        let payload_pointer = proposed.payload[0].as_ptr() as usize;
        let destination = Destination::new();
        let destination_pointer = destination.pointer();
        let refusal =
            refused(transport.try_call_into(proposed, destination, &CallOptions::default()));
        assert!(matches!(refusal.reason, AdmissionError::AtCapacity));
        assert_eq!(
            refusal.request.payload[0].as_ptr() as usize,
            payload_pointer
        );
        assert_eq!(refusal.receiver.pointer(), destination_pointer);
        assert!(matches!(
            payload_dropped.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let mut first = Box::pin(first);
        held(first.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(1));
        write_rpc_message(&mut peer, &reply(Xid(1))).await.unwrap();
        assert!(first.await.result.is_ok());
        assert_eq!(transport.admission_snapshot().calls, 0);
        let retry = accepted(transport.try_call_into(
            refusal.request,
            refusal.receiver,
            &CallOptions::default(),
        ));
        let server = tokio::spawn(async move {
            let call = read_rpc_message(&mut peer).await.unwrap();
            assert_eq!(call.xid, Xid(2));
            if let MessageBody::Call(call) = call.body {
                assert_eq!(call.payload, b"payload!"[..]);
            } else {
                panic!("not call");
            }
            write_rpc_message(&mut peer, &reply(Xid(2))).await.unwrap();
        });
        let outcome = retry.await;
        assert!(outcome.result.is_ok());
        assert_eq!(outcome.receiver.pointer(), destination_pointer);
        assert_eq!(
            &outcome.receiver.bytes[..outcome.receiver.used],
            b"result!!"
        );
        assert_eq!(transport.admission_snapshot().calls, 0);
        server.await.unwrap();
        payload_dropped.recv().await.unwrap();
    })
    .await
}

#[tokio::test]
async fn nonreading_peer_and_many_refusals_do_not_grow_accepted_state_or_emit_frames() {
    bounded(async {
        let (transport, mut peer) = limited(1, 1, usize::MAX);
        let mut active = Box::pin(accepted(transport.try_call_into(
            request(1),
            Destination::new(),
            &CallOptions::default(),
        )));
        held(active.as_mut()).await;
        let admitted = transport.admission_snapshot();
        for xid in 2..202 {
            let refusal = refused(transport.try_call_into(
                request(xid),
                Destination::new(),
                &CallOptions::default(),
            ));
            assert!(matches!(refusal.reason, AdmissionError::AtCapacity));
            assert_eq!(transport.admission_snapshot().calls, admitted.calls);
            assert_eq!(transport.admission_snapshot().bytes, admitted.bytes);
        }
        drop(active);
        assert!(transport.is_closed());
        let mut wire = Vec::new();
        peer.read_to_end(&mut wire).await.unwrap();
        assert_eq!(
            wire.len(),
            1,
            "only the active frame's first byte may be written"
        );
        assert_eq!(transport.admission_snapshot().calls, 0);
    })
    .await
}

#[tokio::test]
async fn legacy_and_owned_calls_share_the_same_connection_budget() {
    bounded(async {
        let (transport, mut peer) = limited(128, 1, usize::MAX);
        let operation = accepted(transport.try_call_into(
            request(1),
            Destination::new(),
            &CallOptions::default(),
        ));
        let error = transport
            .call(request(2).header)
            .await
            .expect_err("legacy must honor busy connection");
        assert!(matches!(error, RuntimeError::Transport(_)));
        let refusal = refused(transport.try_call(request(3).header, &CallOptions::default()));
        assert!(matches!(refusal.reason, AdmissionError::AtCapacity));
        assert_eq!(refusal.request.xid, Xid(3));
        drop(operation);
        let mut legacy = Box::pin(accepted(
            transport.try_call(request(4).header, &CallOptions::default()),
        ));
        held(legacy.as_mut()).await;
        let refusal = refused(transport.try_call_into(
            request(5),
            Destination::new(),
            &CallOptions::default(),
        ));
        assert!(matches!(refusal.reason, AdmissionError::AtCapacity));
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(4));
        write_rpc_message(&mut peer, &reply(Xid(4))).await.unwrap();
        assert!(legacy.await.is_ok());
        assert_eq!(transport.admission_snapshot().calls, 0);
    })
    .await
}

#[tokio::test]
async fn abandoned_sent_call_releases_capacity_before_late_reply() {
    bounded(async {
        let (transport, mut peer) = limited(128, 1, usize::MAX);
        let (drops, mut dropped) = mpsc::unbounded_channel();
        let mut receiver = Destination::new();
        receiver.drops = Some(drops);
        let mut operation = Box::pin(accepted(transport.try_call_into(
            request(1),
            receiver,
            &CallOptions::default(),
        )));
        held(operation.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(1));
        drop(operation);
        dropped
            .recv()
            .await
            .expect("abandoned receive owner released without a reply");
        transport.wait_for_capacity(charge()).await.unwrap();
        assert_eq!(transport.admission_snapshot().calls, 0);
        assert_eq!(transport.admission_snapshot().bytes, 0);
        assert!(!transport.is_closed());
        let mut second = Box::pin(accepted(transport.try_call_into(
            request(2),
            Destination::new(),
            &CallOptions::default(),
        )));
        held(second.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(2));
        write_rpc_message(&mut peer, &reply(Xid(1))).await.unwrap();
        write_rpc_message(&mut peer, &reply(Xid(2))).await.unwrap();
        assert!(second.await.result.is_ok());
        assert_eq!(transport.admission_snapshot().calls, 0);
    })
    .await
}

#[tokio::test]
async fn capacity_wait_is_only_readiness_and_close_wakes_waiters() {
    bounded(async {
        let (transport, _) = limited(128, 1, usize::MAX);
        let operation = accepted(transport.try_call_into(
            request(1),
            Destination::new(),
            &CallOptions::default(),
        ));
        let required = transport.admission_snapshot().bytes;
        let mut first = Box::pin(transport.wait_for_capacity(required));
        let mut second = Box::pin(transport.wait_for_capacity(required));
        held(first.as_mut()).await;
        held(second.as_mut()).await;
        drop(operation);
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(transport.admission_snapshot().calls, 0);
        let operation = accepted(transport.try_call_into(
            request(2),
            Destination::new(),
            &CallOptions::default(),
        ));
        let mut waiting = Box::pin(transport.wait_for_capacity(required));
        held(waiting.as_mut()).await;
        transport.close();
        assert!(waiting.await.is_err());
        let refusal = refused(transport.try_call_into(
            request(3),
            Destination::new(),
            &CallOptions::default(),
        ));
        assert!(matches!(refusal.reason, AdmissionError::Closed));
        drop(operation);
        assert_eq!(transport.admission_snapshot().calls, 0);
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn deadline_while_waiting_for_writer_releases_only_its_permit() {
    let (transport, _peer) = limited(1, 2, usize::MAX);
    let mut active = Box::pin(accepted(transport.try_call_into(
        request(1),
        Destination::new(),
        &CallOptions::default(),
    )));
    held(active.as_mut()).await;
    let options = CallOptions::default().with_timeout(Duration::from_secs(5));
    let waiting = accepted(transport.try_call_into(request(2), Destination::new(), &options));
    let mut waiting = Box::pin(waiting);
    held(waiting.as_mut()).await;
    assert_eq!(transport.admission_snapshot().calls, 2);
    tokio::time::advance(Duration::from_secs(5)).await;
    let outcome = waiting.await;
    assert!(outcome.result.is_err());
    assert!(!transport.is_closed());
    assert_eq!(transport.admission_snapshot().calls, 1);
    drop(active);
    assert_eq!(transport.admission_snapshot().calls, 0);
}

#[tokio::test(start_paused = true)]
async fn call_deadline_during_partial_write_finishes_frame_and_keeps_connection() {
    let (transport, mut peer) = limited(1, 2, usize::MAX);
    let options = CallOptions::default().with_timeout(Duration::from_secs(5));
    let receiver = Destination::new();
    let pointer = receiver.pointer();
    let mut operation = Box::pin(accepted(transport.try_call_into(
        request(1),
        receiver,
        &options,
    )));
    held(operation.as_mut()).await;
    tokio::time::advance(Duration::from_secs(5)).await;
    held(operation.as_mut()).await;
    let (outcome, received) = tokio::join!(operation, read_rpc_message(&mut peer));
    assert_eq!(received.unwrap().xid, Xid(1));
    assert_eq!(
        outcome.result.unwrap_err(),
        RuntimeError::CallTimeout {
            duration: Duration::from_secs(5)
        }
    );
    assert_eq!(outcome.receiver.pointer(), pointer);
    assert!(!transport.is_closed());
    assert_eq!(transport.admission_snapshot().calls, 0);
    let server = tokio::spawn(async move {
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(2));
        write_rpc_message(&mut peer, &reply(Xid(1))).await.unwrap();
        write_rpc_message(&mut peer, &reply(Xid(2))).await.unwrap();
    });
    let next = transport
        .call_into(request(2), outcome.receiver, &CallOptions::default())
        .await;
    assert!(next.result.is_ok());
    assert_eq!(next.receiver.pointer(), pointer);
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reply_deadline_returns_owner_once_without_failing_other_calls() {
    let (transport, mut peer) = limited(128, 2, usize::MAX);
    let options = CallOptions::default().with_timeout(Duration::from_secs(5));
    let (drops, mut dropped) = mpsc::unbounded_channel();
    let mut receiver = Destination::new();
    receiver.drops = Some(drops);
    let pointer = receiver.pointer();
    let mut first = Box::pin(accepted(transport.try_call_into(
        request(1),
        receiver,
        &options,
    )));
    held(first.as_mut()).await;
    read_rpc_message(&mut peer).await.unwrap();
    let second_receiver = Destination::new();
    let second_pointer = second_receiver.pointer();
    let mut second = Box::pin(accepted(transport.try_call_into(
        request(2),
        second_receiver,
        &CallOptions::default(),
    )));
    held(second.as_mut()).await;
    read_rpc_message(&mut peer).await.unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    let first = first.await;
    assert_eq!(
        first.result.as_ref().unwrap_err(),
        &RuntimeError::CallTimeout {
            duration: Duration::from_secs(5)
        }
    );
    assert_eq!(first.receiver.pointer(), pointer);
    assert!(!transport.is_closed());
    assert_eq!(transport.admission_snapshot().calls, 1);
    write_rpc_message(&mut peer, &reply(Xid(1))).await.unwrap();
    write_rpc_message(&mut peer, &reply(Xid(2))).await.unwrap();
    let second = second.await;
    assert!(second.result.is_ok());
    assert_eq!(second.receiver.pointer(), second_pointer);
    assert_eq!(transport.admission_snapshot().calls, 0);
    assert!(matches!(
        dropped.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    drop(first);
    dropped.recv().await.unwrap();
    assert!(matches!(
        dropped.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn invalid_limits_and_request_are_permanent_refusals_without_io() {
    bounded(async {
        for limits in [
            AdmissionLimits {
                max_calls: 0,
                max_bytes: 100,
            },
            AdmissionLimits {
                max_calls: 1,
                max_bytes: 0,
            },
        ] {
            let (stream, _) = duplex(128);
            assert!(
                OwnedRpcTransport::from_stream_with_admission(
                    stream,
                    &config(),
                    RecordLimits::default(),
                    limits
                )
                .is_err()
            );
        }
        let (transport, mut peer) = limited(128, 1, usize::MAX);
        let mut invalid = request(1);
        invalid.header = reply(Xid(1));
        let refusal =
            refused(transport.try_call_into(invalid, Destination::new(), &CallOptions::default()));
        assert!(matches!(refusal.reason, AdmissionError::InvalidRequest(_)));
        assert!(!refusal.reason.is_retryable());
        assert_eq!(transport.admission_snapshot().calls, 0);
        drop(transport);
        let mut wire = Vec::new();
        peer.read_to_end(&mut wire).await.unwrap();
        assert!(wire.is_empty());
    })
    .await
}

#[tokio::test]
async fn completed_unpolled_results_keep_their_admission_until_consumed() {
    struct ReadySignal(tokio::sync::Notify);
    impl std::task::Wake for ReadySignal {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.notify_one();
        }
        fn wake_by_ref(self: &std::sync::Arc<Self>) {
            self.0.notify_one();
        }
    }
    bounded(async {
        for classic in [false, true] {
            let (transport, mut peer) = limited(128, 1, usize::MAX);
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let request = read_rpc_message(&mut peer).await.unwrap();
                write_rpc_message(&mut peer, &reply(request.xid))
                    .await
                    .unwrap();
                stopped.await.unwrap();
            });
            let mut operation: Pin<Box<dyn Future<Output = ()> + Send>> = if classic {
                let call = accepted(transport.try_call(request(1).header, &CallOptions::default()));
                Box::pin(async move {
                    assert!(call.await.is_ok());
                })
            } else {
                let call = accepted(transport.try_call_into(
                    request(1),
                    Destination::new(),
                    &CallOptions::default(),
                ));
                Box::pin(async move {
                    assert!(call.await.result.is_ok());
                })
            };
            let signal = std::sync::Arc::new(ReadySignal(tokio::sync::Notify::new()));
            let waker = std::task::Waker::from(signal.clone());
            let mut context = std::task::Context::from_waker(&waker);
            assert!(operation.as_mut().poll(&mut context).is_pending());
            // The response channel wakes this future only when its outcome is
            // published. Keep it unpolled while inspecting its retained permit.
            signal.0.notified().await;
            assert_eq!(transport.admission_snapshot().calls, 1);
            assert!(!transport.is_closed());
            let refusal = refused(transport.try_call(request(2).header, &CallOptions::default()));
            assert_eq!(refusal.reason, AdmissionError::AtCapacity);
            operation.await;
            assert_eq!(transport.admission_snapshot().calls, 0);
            assert_eq!(transport.admission_snapshot().bytes, 0);
            stop.send(()).unwrap();
            server.await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn abandoned_receiver_destructor_still_holds_its_admission() {
    struct DropProbe {
        destination: Destination,
        transport: OwnedRpcTransport,
        report: Option<tokio::sync::oneshot::Sender<AdmissionError>>,
    }
    impl ReplyReceiver for DropProbe {
        fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
            self.destination.buffer()
        }
        fn advance(&mut self, n: usize) -> Result<(), RuntimeError> {
            self.destination.advance(n)
        }
    }
    impl Drop for DropProbe {
        fn drop(&mut self) {
            let reason = match self
                .transport
                .try_call(request(2).header, &CallOptions::default())
            {
                Err(refusal) => refusal.reason,
                Ok(_) => panic!("capacity released before destination destruction"),
            };
            let _ = self.report.take().unwrap().send(reason);
        }
    }
    bounded(async {
        let (transport, mut peer) = limited(128, 1, usize::MAX);
        let (report, observed) = tokio::sync::oneshot::channel();
        let receiver = DropProbe {
            destination: Destination::new(),
            transport: transport.clone(),
            report: Some(report),
        };
        let mut call = Box::pin(accepted(transport.try_call_into(
            request(1),
            receiver,
            &CallOptions::default(),
        )));
        held(call.as_mut()).await;
        let request = read_rpc_message(&mut peer).await.unwrap();
        let required = transport.admission_snapshot().bytes;
        drop(call);
        assert_eq!(observed.await.unwrap(), AdmissionError::AtCapacity);
        transport.wait_for_capacity(required).await.unwrap();
        assert_eq!(transport.admission_snapshot().calls, 0);
        assert!(!transport.is_closed());
        write_rpc_message(&mut peer, &reply(request.xid))
            .await
            .unwrap();
    })
    .await;
}
