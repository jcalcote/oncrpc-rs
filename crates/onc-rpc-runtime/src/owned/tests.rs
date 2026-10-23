use super::*;
use std::{future::Future, task::Poll};
use tokio::io::duplex;

fn test_transport<S>(
    stream: S,
    call_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
) -> OwnedRpcTransport
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let mut config = ClientConfig::new("127.0.0.1:0".parse().unwrap());
    config.default_call_timeout = call_timeout;
    config.write_timeout = write_timeout;
    OwnedRpcTransport::from_stream(stream, &config).unwrap()
}

fn request(xid: u32) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Call(onc_rpc_wire::CallBody::new(
            ProgramVersion {
                program: 100003,
                version: 3,
            },
            Procedure(1),
            OpaqueAuth::none(),
            OpaqueAuth::none(),
            Bytes::new(),
        )),
    }
}

fn response(xid: u32) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(Bytes::new()),
        })),
    }
}

async fn poll_pending<F: Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    std::future::poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "expected a held operation"
        );
        Poll::Ready(())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn entire_call_timeout_retires_partial_write_and_removes_xid() {
    let (client, mut peer) = duplex(1);
    let transport = test_transport(client, Some(Duration::from_secs(5)), None);
    let mut call = Box::pin(transport.call(request(1)));
    poll_pending(call.as_mut()).await;
    assert_eq!(transport.state.replies().pending.len(), 1);
    // One byte fits: the frame write has started and cannot complete.
    assert!(transport.state.writer.try_lock().is_err());
    tokio::time::advance(Duration::from_secs(5)).await;
    let error = call.await.unwrap_err();
    assert!(error.to_string().contains("call timeout"));
    assert!(transport.state.replies().pending.is_empty());
    assert!(transport.state.replies().closed.is_some());
    let mut remainder = Vec::new();
    peer.read_to_end(&mut remainder).await.unwrap();
    assert_eq!(remainder.len(), 1);
    assert!(transport.call(request(2)).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn waiting_writer_timeout_removes_only_its_xid_and_keeps_channel_healthy() {
    let (client, _peer) = duplex(1);
    let transport = test_transport(client, None, None);
    let mut first = Box::pin(transport.call(request(1)));
    poll_pending(first.as_mut()).await;
    let options = CallOptions::new().with_timeout(Duration::from_secs(1));
    let mut waiting = Box::pin(transport.call_with_options(request(2), &options));
    poll_pending(waiting.as_mut()).await;
    assert_eq!(transport.state.replies().pending.len(), 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(
        waiting
            .await
            .unwrap_err()
            .to_string()
            .contains("call timeout")
    );
    assert!(transport.state.replies().closed.is_none());
    assert_eq!(transport.state.replies().pending.len(), 1);
    assert!(transport.state.replies().pending.contains_key(&Xid(1)));
    drop(first);
    assert!(transport.state.replies().closed.is_some());
    assert!(transport.state.replies().pending.is_empty());
}

#[tokio::test(start_paused = true)]
async fn narrower_write_timeout_retires_interrupted_frame() {
    let (client, _peer) = duplex(1);
    let transport = test_transport(
        client,
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(1)),
    );
    let mut call = Box::pin(transport.call(request(1)));
    poll_pending(call.as_mut()).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(
        call.await
            .unwrap_err()
            .to_string()
            .contains("write timeout")
    );
    assert!(transport.state.replies().closed.is_some());
    assert!(transport.state.replies().pending.is_empty());
}

#[tokio::test(start_paused = true)]
async fn response_timeout_and_cancellation_allow_late_reply_and_connection_reuse() {
    let (client, mut peer) = duplex(4096);
    let transport = test_transport(client, Some(Duration::from_secs(1)), None);
    let mut first = Box::pin(transport.call(request(1)));
    poll_pending(first.as_mut()).await;
    assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(1));
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(
        first
            .await
            .unwrap_err()
            .to_string()
            .contains("call timeout")
    );
    assert!(transport.state.replies().pending.is_empty());
    assert!(transport.state.replies().closed.is_none());
    write_rpc_message(&mut peer, &response(1)).await.unwrap();

    let options = CallOptions::new().without_timeout();
    let mut cancelled = Box::pin(transport.call_with_options(request(2), &options));
    poll_pending(cancelled.as_mut()).await;
    assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(2));
    drop(cancelled);
    assert!(transport.state.replies().pending.is_empty());
    assert!(transport.state.replies().closed.is_none());
    write_rpc_message(&mut peer, &response(2)).await.unwrap();

    let mut final_call = Box::pin(transport.call_with_options(request(3), &options));
    poll_pending(final_call.as_mut()).await;
    assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(3));
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(
        transport.state.replies().pending.len(),
        1,
        "without_timeout must override the configured default"
    );
    write_rpc_message(&mut peer, &response(3)).await.unwrap();
    assert_eq!(final_call.await.unwrap().xid, Xid(3));
    assert!(transport.state.replies().pending.is_empty());
}

#[tokio::test]
async fn eof_fails_all_multiplexed_calls_and_future_calls_without_leaked_xids() {
    let (client, peer) = duplex(4096);
    let transport = test_transport(client, None, None);
    let mut first = Box::pin(transport.call(request(1)));
    let mut second = Box::pin(transport.call(request(2)));
    poll_pending(first.as_mut()).await;
    poll_pending(second.as_mut()).await;
    assert_eq!(transport.state.replies().pending.len(), 2);
    drop(peer);
    assert_eq!(first.await.unwrap_err(), RuntimeError::ConnectionClosed);
    assert_eq!(second.await.unwrap_err(), RuntimeError::ConnectionClosed);
    assert!(transport.state.replies().pending.is_empty());
    assert_eq!(
        transport.call(request(3)).await.unwrap_err(),
        RuntimeError::ConnectionClosed
    );
}

#[tokio::test]
async fn last_transport_drop_aborts_weak_reader_and_releases_stream() {
    let (client, mut peer) = duplex(4096);
    let transport = test_transport(client, None, None);
    let owner = Arc::downgrade(&transport.state);
    drop(transport);
    assert!(owner.upgrade().is_none());
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), peer.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn duplicate_inflight_xid_cannot_replace_existing_owner() {
    let (client, mut peer) = duplex(4096);
    let transport = test_transport(client, None, None);
    let mut first = Box::pin(transport.call(request(1)));
    poll_pending(first.as_mut()).await;
    assert!(
        transport
            .call(request(1))
            .await
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    assert_eq!(transport.state.replies().pending.len(), 1);
    read_rpc_message(&mut peer).await.unwrap();
    write_rpc_message(&mut peer, &response(1)).await.unwrap();
    assert_eq!(first.await.unwrap().xid, Xid(1));
}

#[tokio::test]
async fn records_are_bounded_before_payload_allocation_and_fragments_are_supported() {
    let oversized = (0x8000_0000_u32 | u32::try_from(MAX_RECORD_BYTES).unwrap()).to_be_bytes();
    let error = read_rpc_message(&mut oversized.as_slice())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exceeds configured limit"));
    let fragments = vec![0_u8; 4 * MAX_FRAGMENTS];
    assert!(
        read_rpc_message(&mut fragments.as_slice())
            .await
            .unwrap_err()
            .to_string()
            .contains("too many fragments")
    );
    let payload = response(7).encode().unwrap();
    let mut fragmented = Vec::from(4_u32.to_be_bytes());
    fragmented.extend_from_slice(&payload[..4]);
    fragmented.extend_from_slice(
        &(0x8000_0000_u32 | u32::try_from(payload.len() - 4).unwrap()).to_be_bytes(),
    );
    fragmented.extend_from_slice(&payload[4..]);
    assert_eq!(
        read_rpc_message(&mut fragmented.as_slice()).await.unwrap(),
        response(7)
    );
    let mut truncated = &fragmented[..fragmented.len() - 1];
    assert_eq!(
        read_rpc_message(&mut truncated).await.unwrap_err(),
        RuntimeError::ConnectionClosed
    );
}

#[tokio::test]
async fn explicit_close_interrupts_blocked_writer_and_fails_pending_calls() {
    let (client, mut peer) = duplex(1);
    let transport = test_transport(client, None, None);
    let clone = transport.clone();
    let mut call = Box::pin(transport.call(request(1)));
    poll_pending(call.as_mut()).await;
    assert!(transport.state.writer.try_lock().is_err());
    clone.close();
    clone.close();
    assert!(transport.is_closed());
    assert_eq!(call.await.unwrap_err(), RuntimeError::ConnectionClosed);
    assert!(transport.state.replies().pending.is_empty());
    let mut remaining = Vec::new();
    peer.read_to_end(&mut remaining).await.unwrap();
    assert_eq!(remaining.len(), 1);
}

#[tokio::test]
async fn dropping_cache_owner_preserves_borrowed_call_until_terminal_reply() {
    let (client, mut peer) = duplex(4096);
    let transport = test_transport(client, None, None);
    let lease = transport.clone();
    let owner = Arc::downgrade(&transport.state);
    let task = tokio::spawn(async move { lease.call(request(1)).await });
    assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(1));
    drop(transport);
    assert!(owner.upgrade().is_some());
    write_rpc_message(&mut peer, &response(1)).await.unwrap();
    assert_eq!(task.await.unwrap().unwrap().xid, Xid(1));
    let mut byte = [0];
    assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
    assert!(owner.upgrade().is_none());
}

#[tokio::test]
async fn custom_record_limits_fail_before_accepting_oversized_payload() {
    let limits = RecordLimits {
        max_record_bytes: 64,
        max_fragments: 2,
    };
    let mut record = (0x8000_0000_u32 | 65).to_be_bytes().as_slice().to_owned();
    assert!(
        read_rpc_message_with_limits(&mut record.as_slice(), limits)
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeds configured limit")
    );
    record.clear();
    assert!(
        read_rpc_message_with_limits(
            &mut record.as_slice(),
            RecordLimits {
                max_record_bytes: 0,
                max_fragments: 0
            }
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn pending_sender_waker_can_reenter_connection_cleanup_without_registry_lock() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Reentrant {
        state: Weak<State>,
        unlocked: AtomicBool,
    }
    impl std::task::Wake for Reentrant {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            let state = self.state.upgrade().unwrap();
            let unlocked = state.replies.try_lock().is_ok();
            self.unlocked.store(unlocked, Ordering::SeqCst);
            if unlocked {
                state.fail(RuntimeError::ConnectionClosed);
            }
        }
    }
    let (stream, _peer) = duplex(128);
    let transport = test_transport(stream, None, None);
    let token = Arc::new(());
    let permit = Arc::new(transport.state.admission.try_acquire(128).unwrap());
    let (sender, receiver) = oneshot::channel();
    transport.state.replies().pending.insert(
        Xid(99),
        Pending {
            token: token.clone(),
            sender: Some(ResponseSender::Buffered { sender, permit }),
        },
    );
    let pending = PendingCall {
        retain_reply: false,
        state: transport.state.clone(),
        xid: Xid(99),
        token,
    };
    let signal = Arc::new(Reentrant {
        state: Arc::downgrade(&transport.state),
        unlocked: AtomicBool::new(false),
    });
    let waker = std::task::Waker::from(signal.clone());
    let mut context = std::task::Context::from_waker(&waker);
    let mut receiver = Box::pin(receiver);
    assert!(receiver.as_mut().poll(&mut context).is_pending());
    drop(pending);
    assert!(
        signal.unlocked.load(Ordering::SeqCst),
        "sender destruction invoked a waker under the reply registry lock"
    );
    assert!(
        transport.is_closed(),
        "the waker reentered connection cleanup"
    );
    assert!(receiver.await.is_err());
    assert_eq!(transport.admission_snapshot().calls, 0);
}
