//! Behavioral contract for the opt-in owned-buffer transport path.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClientTransport, CallOptions, ClientConfig, MessageBody,
    OpaqueAuth, OwnedRpcTransport, Procedure, ProgramVersion, RecordLimits, RejectedReply,
    ReplyBody, RpcMessage, SegmentedRequest, Xid, fragment_record, read_rpc_message,
    write_rpc_message,
};
use onc_rpc_wire::CallBody;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf, duplex,
};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

fn config() -> ClientConfig {
    ClientConfig::new("127.0.0.1:0".parse().unwrap())
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
        payload: vec![
            Bytes::from_static(b"abc"),
            Bytes::new(),
            Bytes::from_static(b"defgh"),
        ],
    }
}
fn reply(xid: u32, data: &'static [u8]) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(Bytes::from_static(data)),
        })),
    }
}
fn transport(capacity: usize) -> (OwnedRpcTransport, DuplexStream) {
    let (client, peer) = duplex(capacity);
    (
        OwnedRpcTransport::from_stream(client, &config()).unwrap(),
        peer,
    )
}
async fn held<F: Future>(future: std::pin::Pin<&mut F>) {
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
        .expect("test operation exceeded 10-second hang bound")
}

#[tokio::test]
async fn fragmented_reply_fills_the_original_segments_and_preserves_tail() {
    bounded(async {
        let (transport, mut peer) = transport(3);
        let buffers = vec![vec![0xaa; 2], vec![], vec![0xbb; 9]];
        let pointers: Vec<_> = buffers.iter().map(|b| b.as_ptr() as usize).collect();
        let server = tokio::spawn(async move {
            let call = read_rpc_message(&mut peer).await.unwrap();
            match call.body {
                MessageBody::Call(body) => assert_eq!(body.payload, b"abcdefgh"[..]),
                _ => panic!("not a call"),
            }
            for fragment in fragment_record(&reply(11, b"12345678").encode().unwrap(), 3).unwrap() {
                peer.write_all(&fragment).await.unwrap();
            }
        });
        let result = transport
            .call_segmented(request(11), buffers, &CallOptions::default())
            .await
            .unwrap();
        assert_eq!(result.payload_len, 8);
        assert_eq!(result.buffers[0], b"12");
        assert_eq!(
            result.buffers[2],
            [b'3', b'4', b'5', b'6', b'7', b'8', 0xbb, 0xbb, 0xbb]
        );
        assert_eq!(
            result
                .buffers
                .iter()
                .map(|b| b.as_ptr() as usize)
                .collect::<Vec<_>>(),
            pointers
        );
        assert_eq!(result.header, reply(11, b""));
        server.await.unwrap();
    })
    .await
}

#[tokio::test]
async fn concurrent_segmented_and_legacy_calls_match_reverse_order_replies() {
    bounded(async {
        let (transport, mut peer) = transport(8);
        let server = tokio::spawn(async move {
            let a = read_rpc_message(&mut peer).await.unwrap();
            let b = read_rpc_message(&mut peer).await.unwrap();
            for xid in [b.xid.0, a.xid.0] {
                write_rpc_message(
                    &mut peer,
                    &reply(xid, if xid == 1 { b"one" } else { b"two" }),
                )
                .await
                .unwrap();
            }
        });
        let options = CallOptions::default();
        let (a, b) = tokio::join!(
            transport.call_segmented(request(1), vec![vec![0; 3]], &options),
            transport.call(request(2).header)
        );
        let a = a.unwrap();
        assert_eq!(a.buffers[0], b"one");
        assert_eq!(a.header.xid, Xid(1));
        assert_eq!(b.unwrap(), reply(2, b"two"));
        server.await.unwrap();
    })
    .await
}

#[derive(Debug)]
struct TrackedBuffer {
    bytes: Vec<u8>,
    drops: mpsc::UnboundedSender<()>,
    writes: Arc<AtomicUsize>,
}
impl AsMut<[u8]> for TrackedBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        self.writes.fetch_add(1, Ordering::SeqCst);
        &mut self.bytes
    }
}
impl Drop for TrackedBuffer {
    fn drop(&mut self) {
        let _ = self.drops.send(());
    }
}
fn tracked() -> (TrackedBuffer, mpsc::UnboundedReceiver<()>, Arc<AtomicUsize>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let writes = Arc::new(AtomicUsize::new(0));
    (
        TrackedBuffer {
            bytes: vec![0; 8],
            drops: tx,
            writes: writes.clone(),
        },
        rx,
        writes,
    )
}

#[tokio::test]
async fn dropping_sent_call_releases_buffers_before_late_reply_and_preserves_connection() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let (buffer, mut drops, writes) = tracked();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_segmented(request(31), vec![buffer], &options));
        held(call.as_mut()).await;
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(31));
        drop(call);
        // Destination destruction is a readiness event: it must happen before
        // the peer sends its late response, not be deferred until that response.
        drops
            .recv()
            .await
            .expect("abandoned destination released promptly");
        let accesses_after_release = writes.load(Ordering::SeqCst);
        assert!(!transport.is_closed());
        write_rpc_message(&mut peer, &reply(31, b"late"))
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            read_rpc_message(&mut peer).await.unwrap();
            write_rpc_message(&mut peer, &reply(32, b"next"))
                .await
                .unwrap();
        });
        assert_eq!(
            transport
                .call_segmented(request(32), vec![vec![0; 4]], &options)
                .await
                .unwrap()
                .buffers[0],
            b"next"
        );
        assert_eq!(writes.load(Ordering::SeqCst), accesses_after_release);
        server.await.unwrap();
    })
    .await
}

#[tokio::test]
async fn dropping_during_reply_detaches_buffer_before_remaining_fragments() {
    bounded(async {
        let encoded = reply(41, b"abcdefgh").encode().unwrap();
        let split = encoded.len() - 6;
        let (client, mut peer) = duplex(128);
        let (seen, observed) = oneshot::channel();
        let stream = ObservedStream {
            inner: client,
            read_bytes: 0,
            signal_at: split + 4,
            seen: Some(seen),
        };
        let transport = OwnedRpcTransport::from_stream(stream, &config()).unwrap();
        let (buffer, mut drops, writes) = tracked();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_segmented(request(41), vec![buffer], &options));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        // Deliver the envelope and two result bytes in a nonfinal fragment.
        peer.write_all(&(split as u32).to_be_bytes()).await.unwrap();
        peer.write_all(&encoded[..split]).await.unwrap();
        observed
            .await
            .expect("reader consumed the nonfinal fragment");
        drop(call);
        drops
            .recv()
            .await
            .expect("active read borrow quiesced before destination drop");
        let accesses_after_release = writes.load(Ordering::SeqCst);
        peer.write_all(&(0x8000_0000u32 | 6).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&encoded[split..]).await.unwrap();
        // A subsequent reply is the completion barrier for draining both late
        // fragments. The abandoned allocation must never be exposed again.
        let server = tokio::spawn(async move {
            assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(42));
            write_rpc_message(&mut peer, &reply(42, b"next"))
                .await
                .unwrap();
        });
        let next = transport
            .call_segmented(request(42), vec![vec![0; 4]], &options)
            .await
            .unwrap();
        assert_eq!(next.buffers[0], b"next");
        assert!(accesses_after_release > 0);
        assert_eq!(writes.load(Ordering::SeqCst), accesses_after_release);
        server.await.unwrap();
    })
    .await
}

#[tokio::test]
async fn insufficient_capacity_drains_record_before_following_reply() {
    bounded(async {
        let (transport, mut peer) = transport(8);
        let server = tokio::spawn(async move {
            let a = read_rpc_message(&mut peer).await.unwrap();
            let b = read_rpc_message(&mut peer).await.unwrap();
            write_rpc_message(&mut peer, &reply(a.xid.0, b"too large"))
                .await
                .unwrap();
            write_rpc_message(&mut peer, &reply(b.xid.0, b"ok"))
                .await
                .unwrap();
        });
        let options = CallOptions::default();
        let (a, b) = tokio::join!(
            transport.call_segmented(request(51), vec![vec![0; 2]], &options),
            transport.call_segmented(request(52), vec![vec![0; 16]], &options)
        );
        assert!(
            a.expect_err("capacity error")
                .to_string()
                .contains("capacity")
        );
        let b = b.unwrap();
        assert_eq!(b.payload_len, 2);
        assert_eq!(&b.buffers[0][..2], b"ok");
        server.await.unwrap();
    })
    .await
}

#[tokio::test]
async fn remote_denial_and_accept_failure_are_preserved_without_payload_capacity() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let statuses = vec![
            ReplyBody::Denied(RejectedReply::AuthError(onc_rpc_runtime::AuthStat::BadCred)),
            ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::ProcedureUnavailable,
            }),
        ];
        for (i, body) in statuses.into_iter().enumerate() {
            let xid = 61 + i as u32;
            let expected = RpcMessage {
                xid: Xid(xid),
                body: MessageBody::Reply(body),
            };
            let options = CallOptions::default();
            let mut call =
                Box::pin(transport.call_segmented(request(xid), Vec::<Vec<u8>>::new(), &options));
            held(call.as_mut()).await;
            read_rpc_message(&mut peer).await.unwrap();
            write_rpc_message(&mut peer, &expected).await.unwrap();
            let result = call.await.unwrap();
            assert_eq!(result.header, expected);
            assert_eq!(result.payload_len, 0);
        }
    })
    .await
}

#[tokio::test]
async fn truncated_response_fails_and_releases_receive_storage() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let (buffer, mut drops, _) = tracked();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_segmented(request(71), vec![buffer], &options));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        peer.write_all(&(0x8000_0000u32 | 32).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&71u32.to_be_bytes()).await.unwrap();
        drop(peer);
        assert!(call.await.is_err());
        drops.recv().await.unwrap();
        assert!(transport.is_closed());
    })
    .await
}

#[tokio::test]
async fn record_limit_is_checked_before_receiving_oversized_fragment() {
    bounded(async {
        let (client, mut peer) = duplex(128);
        let transport = OwnedRpcTransport::from_stream_with_limits(
            client,
            &config(),
            RecordLimits {
                max_record_bytes: 64,
                max_fragments: 4,
            },
        )
        .unwrap();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_segmented(request(81), vec![vec![0; 16]], &options));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        peer.write_all(&(0x8000_0000u32 | 100).to_be_bytes())
            .await
            .unwrap();
        assert!(
            call.await
                .expect_err("limit error")
                .to_string()
                .contains("limit")
        );
        assert!(transport.is_closed());
    })
    .await
}

#[tokio::test]
async fn last_transport_drop_quiesces_an_abandoned_receive() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let (buffer, mut drops, _) = tracked();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_segmented(request(91), vec![buffer], &options));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        drop(call);
        drop(transport);
        drops
            .recv()
            .await
            .expect("last owner released receive storage");
        let mut trailing = Vec::new();
        peer.read_to_end(&mut trailing).await.unwrap();
        assert!(trailing.is_empty());
    })
    .await
}

#[tokio::test]
async fn invalid_request_is_rejected_before_any_frame_is_emitted() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let mut req = request(101);
        if let MessageBody::Call(body) = &mut req.header.body {
            body.payload = Bytes::from_static(b"not empty");
        }
        assert!(
            transport
                .call_segmented(req, vec![vec![0; 8]], &CallOptions::default())
                .await
                .is_err()
        );
        drop(transport);
        let mut trailing = Vec::new();
        peer.read_to_end(&mut trailing).await.unwrap();
        assert!(trailing.is_empty());
    })
    .await
}

#[repr(align(4096))]
struct AlignedPage([u8; 4096]);
struct AlignedOwned(Box<AlignedPage>);
impl AsMut<[u8]> for AlignedOwned {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0.0
    }
}

#[tokio::test]
async fn page_aligned_custom_allocation_is_returned_without_replacement() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let buffer = AlignedOwned(Box::new(AlignedPage([0xab; 4096])));
        let pointer = buffer.0.0.as_ptr() as usize;
        assert_eq!(pointer % 4096, 0);
        let server = tokio::spawn(async move {
            read_rpc_message(&mut peer).await.unwrap();
            write_rpc_message(&mut peer, &reply(111, b"aligned payload"))
                .await
                .unwrap();
        });
        let mut result = transport
            .call_segmented(request(111), vec![buffer], &CallOptions::default())
            .await
            .unwrap();
        assert_eq!(result.payload_len, 15);
        assert_eq!(result.buffers[0].0.0.as_ptr() as usize, pointer);
        assert_eq!(&result.buffers[0].as_mut()[..15], b"aligned payload");
        assert_eq!(result.buffers[0].as_mut()[15], 0xab);
        server.await.unwrap();
    })
    .await
}

struct ObservedStream {
    inner: DuplexStream,
    read_bytes: usize,
    signal_at: usize,
    seen: Option<oneshot::Sender<()>>,
}
impl AsyncRead for ObservedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = output.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, output);
        self.read_bytes += output.filled().len() - before;
        if self.read_bytes >= self.signal_at {
            if let Some(seen) = self.seen.take() {
                let _ = seen.send(());
            }
        }
        result
    }
}
impl AsyncWrite for ObservedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn canceling_a_partial_segmented_write_retires_the_connection() {
    bounded(async {
        let (transport, mut peer) = transport(1);
        let (buffer, mut drops, _) = tracked();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_segmented(request(121), vec![buffer], &options));
        held(call.as_mut()).await;
        drop(call);
        assert!(transport.is_closed());
        drops.recv().await.unwrap();
        let mut partial = Vec::new();
        peer.read_to_end(&mut partial).await.unwrap();
        assert_eq!(partial.len(), 1);
        assert!(
            transport
                .call_segmented(request(122), vec![vec![0; 8]], &options)
                .await
                .is_err()
        );
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn reply_timeout_quiesces_storage_and_preserves_connection() {
    let (transport, mut peer) = transport(128);
    let (buffer, mut drops, _) = tracked();
    let options = CallOptions::default().with_timeout(Duration::from_secs(5));
    let mut call = Box::pin(transport.call_segmented(request(131), vec![buffer], &options));
    held(call.as_mut()).await;
    read_rpc_message(&mut peer).await.unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(
        call.await.expect_err("deadline expired"),
        onc_rpc_runtime::RuntimeError::CallTimeout {
            duration: Duration::from_secs(5)
        }
    );
    assert!(!transport.is_closed());
    drops.recv().await.unwrap();
    // No reply was needed to release the old destination. Drain it now and
    // demonstrate that another call still succeeds on the same stream.
    write_rpc_message(&mut peer, &reply(131, b"late"))
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(132));
        write_rpc_message(&mut peer, &reply(132, b"next"))
            .await
            .unwrap();
    });
    let next = transport
        .call_segmented(request(132), vec![vec![0; 4]], &CallOptions::default())
        .await
        .unwrap();
    assert_eq!(next.buffers[0], b"next");
    server.await.unwrap();
}

#[tokio::test]
async fn buffered_stream_is_flushed_before_waiting_for_reply() {
    bounded(async {
        let (client, mut peer) = duplex(4096);
        let buffered = tokio::io::BufStream::new(client);
        let transport = OwnedRpcTransport::from_stream(buffered, &config()).unwrap();
        let server = tokio::spawn(async move {
            assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(91));
            write_rpc_message(&mut peer, &reply(91, b"ok"))
                .await
                .unwrap();
        });
        let result = transport
            .call_segmented(request(91), vec![vec![0; 2]], &CallOptions::default())
            .await
            .unwrap();
        assert_eq!(result.buffers[0], b"ok");
        server.await.unwrap();
    })
    .await
}

#[tokio::test]
async fn malformed_envelopes_keep_wire_errors_and_retire_connection() {
    bounded(async {
        use onc_rpc_runtime::{RuntimeError, WireError};
        let original = reply(92, b"").encode().unwrap();
        let mut cases = Vec::new();
        for (offset, word, expected) in [
            (4, 7u32, WireError::InvalidMessageType(7)),
            (8, 7u32, WireError::InvalidReplyStat(7)),
            (20, 7u32, WireError::InvalidAcceptStat(7)),
            (16, 401u32, WireError::AuthBodyTooLong(401)),
        ] {
            let mut bytes = original.to_vec();
            bytes[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
            cases.push((bytes, expected));
        }
        let mut error_reply = reply(92, b"");
        if let MessageBody::Reply(ReplyBody::Accepted(body)) = &mut error_reply.body {
            body.status = AcceptedStatus::ProgramUnavailable;
        }
        let mut bytes = error_reply.encode().unwrap().to_vec();
        bytes.extend_from_slice(b"bad");
        cases.push((bytes, WireError::TrailingBytes(3)));
        let mut padded = reply(92, b"");
        if let MessageBody::Reply(ReplyBody::Accepted(body)) = &mut padded.body {
            body.verifier = OpaqueAuth::new(
                onc_rpc_wire::AuthFlavor::Unknown(99),
                Bytes::from_static(b"v"),
            )
            .unwrap();
        }
        let mut bytes = padded.encode().unwrap().to_vec();
        bytes[21] = 1;
        cases.push((bytes, WireError::NonZeroXdrPadding));
        for (bytes, expected) in cases {
            let (transport, mut peer) = transport(4096);
            let server = tokio::spawn(async move {
                read_rpc_message(&mut peer).await.unwrap();
                for fragment in fragment_record(&bytes, 3).unwrap() {
                    // The reader may close as soon as the invalid prefix is seen.
                    if peer.write_all(&fragment).await.is_err() {
                        break;
                    }
                }
            });
            let error = transport
                .call_segmented(request(92), vec![vec![0; 8]], &CallOptions::default())
                .await
                .err()
                .unwrap();
            assert_eq!(error, RuntimeError::Wire(expected));
            assert!(transport.is_closed());
            server.await.unwrap();
        }
    })
    .await
}

#[tokio::test]
async fn maximum_verifier_and_error_range_fit_bounded_envelope() {
    bounded(async {
        let (transport, mut peer) = transport(17);
        let expected = RpcMessage {
            xid: Xid(93),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::new(
                    onc_rpc_wire::AuthFlavor::Unknown(99),
                    Bytes::from(vec![7; 400]),
                )
                .unwrap(),
                status: AcceptedStatus::ProgramMismatch(onc_rpc_runtime::VersionRange {
                    low: 1,
                    high: 9,
                }),
            })),
        };
        let sent = expected.clone();
        let server = tokio::spawn(async move {
            read_rpc_message(&mut peer).await.unwrap();
            for fragment in fragment_record(&sent.encode().unwrap(), 1).unwrap() {
                peer.write_all(&fragment).await.unwrap();
            }
        });
        let result = transport
            .call_segmented(request(93), Vec::<Vec<u8>>::new(), &CallOptions::default())
            .await
            .unwrap();
        assert_eq!(result.header, expected);
        assert_eq!(result.payload_len, 0);
        server.await.unwrap();
    })
    .await
}

#[tokio::test]
async fn canceling_before_writer_admission_releases_unsent_destinations() {
    bounded(async {
        let (transport, _peer) = transport(1);
        let first_options = CallOptions::default();
        let mut first = Box::pin(transport.call(request(94).header));
        held(first.as_mut()).await;
        let (buffer, mut drops, _) = tracked();
        let mut queued =
            Box::pin(transport.call_segmented(request(95), vec![buffer], &first_options));
        held(queued.as_mut()).await;
        drop(queued);
        drops
            .recv()
            .await
            .expect("unsent buffer should be released");
        assert!(!transport.is_closed());
        drop(first);
        assert!(transport.is_closed());
    })
    .await
}
