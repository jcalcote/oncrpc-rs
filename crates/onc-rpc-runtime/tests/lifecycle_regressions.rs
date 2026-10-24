//! Deterministic cleanup regressions for buffered calls and untouched writers.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClientTransport, CallCancellation, CallOptions,
    ClientConfig, MessageBody, OpaqueAuth, OwnedRpcTransport, Procedure, ProgramVersion, ReplyBody,
    RpcMessage, RuntimeError, Xid, read_rpc_message, write_rpc_message,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf, duplex},
    sync::oneshot,
};

fn request(xid: u32) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Call(onc_rpc_wire::CallBody::new(
            ProgramVersion {
                program: 42,
                version: 1,
            },
            Procedure(1),
            OpaqueAuth::none(),
            OpaqueAuth::none(),
            Bytes::new(),
        )),
    }
}
fn reply(xid: u32) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(Bytes::from_static(b"ok")),
        })),
    }
}
fn config() -> ClientConfig {
    ClientConfig::new("127.0.0.1:0".parse().unwrap())
}
async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("lifecycle cleanup hang guard")
}
async fn held<F: Future>(mut future: Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

struct ObservedStream {
    inner: DuplexStream,
    read_bytes: usize,
    body_read: Option<oneshot::Sender<()>>,
    write_attempted: Option<oneshot::Sender<()>>,
    write_ready: Arc<AtomicBool>,
    write_waker: Arc<Mutex<Option<Waker>>>,
}
impl AsyncRead for ObservedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // A body-read attempt follows the marker and XID reads. The reader has
        // already taken the buffered target from the connection registry.
        if self.read_bytes >= 8 {
            if let Some(ready) = self.body_read.take() {
                let _ = ready.send(());
            }
        }
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        self.read_bytes += buffer.filled().len() - before;
        result
    }
}
impl AsyncWrite for ObservedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !self.write_ready.load(Ordering::Acquire) {
            *self.write_waker.lock().unwrap() = Some(cx.waker().clone());
            if let Some(ready) = self.write_attempted.take() {
                let _ = ready.send(());
            }
            return Poll::Pending;
        }
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
async fn closing_during_an_active_buffered_reply_returns_error_without_panicking() {
    bounded(async {
        let (stream, mut peer) = duplex(4096);
        let (body_read, body_ready) = oneshot::channel();
        let observed = ObservedStream {
            inner: stream,
            read_bytes: 0,
            body_read: Some(body_read),
            write_attempted: None,
            write_ready: Arc::new(AtomicBool::new(true)),
            write_waker: Arc::new(Mutex::new(None)),
        };
        let transport = OwnedRpcTransport::from_stream(observed, &config()).unwrap();
        let mut call = Box::pin(transport.call(request(1)));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        let encoded = reply(1).encode().unwrap();
        peer.write_all(&(0x8000_0000 | encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&encoded[..4]).await.unwrap();
        body_ready.await.unwrap();
        transport.close();
        assert_eq!(call.await, Err(RuntimeError::ConnectionClosed));
    })
    .await;
}

#[tokio::test]
async fn cancellation_before_the_first_actual_write_preserves_the_stream() {
    bounded(async {
        let (stream, mut peer) = duplex(4096);
        let write_ready = Arc::new(AtomicBool::new(false));
        let write_waker = Arc::new(Mutex::new(None));
        let (write_attempted, attempted) = oneshot::channel();
        let observed = ObservedStream {
            inner: stream,
            read_bytes: 0,
            body_read: None,
            write_attempted: Some(write_attempted),
            write_ready: write_ready.clone(),
            write_waker: write_waker.clone(),
        };
        let transport = OwnedRpcTransport::from_stream(observed, &config()).unwrap();
        let cancellation = CallCancellation::new();
        let options = CallOptions::new().with_cancellation(cancellation.clone());
        let mut call = Box::pin(transport.call_with_options(request(1), &options));
        tokio::select! {
            result = &mut call => panic!("gated call completed: {result:?}"),
            ready = attempted => { ready.unwrap(); }
        }
        cancellation.cancel();
        assert_eq!(call.await, Err(RuntimeError::Cancelled));
        assert!(!transport.is_closed());
        write_ready.store(true, Ordering::Release);
        let waker = write_waker.lock().unwrap().take();
        if let Some(waker) = waker {
            waker.wake();
        }
        let (release_peer, peer_released) = oneshot::channel();
        let server = tokio::spawn(async move {
            let request = read_rpc_message(&mut peer).await.unwrap();
            assert_eq!(request.xid, Xid(2));
            write_rpc_message(&mut peer, &reply(2)).await.unwrap();
            peer_released.await.unwrap();
        });
        assert_eq!(transport.call(request(2)).await.unwrap(), reply(2));
        assert!(!transport.is_closed());
        release_peer.send(()).unwrap();
        server.await.unwrap();
    })
    .await;
}
