//! Owned outcome and application-directed receive contract.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, CallOptions, ClientConfig, MessageBody, OpaqueAuth,
    OwnedRpcTransport, Procedure, ProgramVersion, ReplyBody, ReplyReceiver, RpcMessage,
    RuntimeError, SegmentedRequest, Xid, fragment_record, read_rpc_message, write_rpc_message,
};
use onc_rpc_wire::CallBody;
use std::{future::Future, task::Poll, time::Duration};
use tokio::io::{AsyncWriteExt, DuplexStream, duplex};
use tokio::sync::oneshot;

fn request(xid: u32) -> SegmentedRequest {
    SegmentedRequest {
        header: RpcMessage {
            xid: Xid(xid),
            body: MessageBody::Call(CallBody::new(
                ProgramVersion {
                    program: 100003,
                    version: 3,
                },
                Procedure(6),
                OpaqueAuth::none(),
                OpaqueAuth::none(),
                Bytes::new(),
            )),
        },
        payload: Vec::new(),
    }
}
fn reply(xid: u32, payload: &[u8]) -> RpcMessage {
    RpcMessage {
        xid: Xid(xid),
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(Bytes::copy_from_slice(payload)),
        })),
    }
}
fn transport(capacity: usize) -> (OwnedRpcTransport, DuplexStream) {
    let (stream, peer) = duplex(capacity);
    (
        OwnedRpcTransport::from_stream(stream, &ClientConfig::new("127.0.0.1:0".parse().unwrap()))
            .unwrap(),
        peer,
    )
}
async fn held<F: Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}
async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("owned receive hang bound")
}

#[derive(Debug)]
struct Destination {
    bytes: Vec<u8>,
    offset: usize,
    on_buffer: Option<oneshot::Sender<()>>,
    fail_advance: bool,
    panic_advance: bool,
}
impl Destination {
    fn new(length: usize) -> Self {
        Self {
            bytes: vec![0; length],
            offset: 0,
            on_buffer: None,
            fail_advance: false,
            panic_advance: false,
        }
    }
}
impl ReplyReceiver for Destination {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        if let Some(sender) = self.on_buffer.take() {
            let _ = sender.send(());
        }
        Ok(&mut self.bytes[self.offset..])
    }
    fn advance(&mut self, bytes: usize) -> Result<(), RuntimeError> {
        self.offset += bytes;
        assert!(!self.panic_advance, "intentional receiver callback panic");
        if self.fail_advance {
            return Err(RuntimeError::Transport(
                "application prefix rejected".into(),
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn validation_duplicate_and_closed_failures_return_original_destination() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let mut invalid = request(1);
        if let MessageBody::Call(call) = &mut invalid.header.body {
            call.payload = Bytes::from_static(b"invalid");
        }
        let destination = Destination::new(8);
        let pointer = destination.bytes.as_ptr();
        let result = transport
            .call_into(invalid, destination, &CallOptions::default())
            .await;
        assert!(
            result
                .result
                .unwrap_err()
                .to_string()
                .contains("empty call envelope")
        );
        assert_eq!(result.receiver.bytes.as_ptr(), pointer);
        assert_eq!(result.payload_len, 0);
        let options = CallOptions::default();
        let mut first = Box::pin(transport.call_into(request(2), result.receiver, &options));
        held(first.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        let duplicate = Destination::new(8);
        let duplicate_pointer = duplicate.bytes.as_ptr();
        let result = transport.call_into(request(2), duplicate, &options).await;
        assert!(result.result.unwrap_err().to_string().contains("duplicate"));
        assert_eq!(result.receiver.bytes.as_ptr(), duplicate_pointer);
        assert!(!transport.is_closed());
        transport.close();
        let first = first.await;
        assert!(first.result.is_err());
        assert_eq!(first.receiver.bytes.as_ptr(), pointer);
        let closed = transport
            .call_into(request(3), result.receiver, &options)
            .await;
        assert!(closed.result.is_err());
        assert_eq!(closed.receiver.bytes.as_ptr(), duplicate_pointer);
    })
    .await;
}

#[tokio::test]
async fn eof_returns_original_destination_with_actual_partial_count() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let destination = Destination::new(8);
        let pointer = destination.bytes.as_ptr();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_into(request(11), destination, &options));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        let encoded = reply(11, b"abcdef").encode().unwrap();
        peer.write_all(&(0x8000_0000 | encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&encoded[..encoded.len() - 4]).await.unwrap();
        drop(peer);
        let outcome = call.await;
        assert_eq!(outcome.result.unwrap_err(), RuntimeError::ConnectionClosed);
        assert_eq!(outcome.receiver.bytes.as_ptr(), pointer);
        assert_eq!(outcome.payload_len, 2);
        assert_eq!(outcome.receiver.offset, 2);
        assert_eq!(&outcome.receiver.bytes[..2], b"ab");
        assert!(transport.is_closed());
    })
    .await;
}

#[tokio::test]
async fn capacity_failure_returns_owner_and_keeps_following_record_usable() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let server = tokio::spawn(async move {
            for xid in [21, 22] {
                assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(xid));
                write_rpc_message(
                    &mut peer,
                    &reply(xid, if xid == 21 { b"large" } else { b"ok" }),
                )
                .await
                .unwrap();
            }
        });
        let destination = Destination::new(2);
        let pointer = destination.bytes.as_ptr();
        let first = transport
            .call_into(request(21), destination, &CallOptions::default())
            .await;
        assert!(first.result.unwrap_err().to_string().contains("capacity"));
        assert_eq!(first.receiver.bytes.as_ptr(), pointer);
        assert_eq!(first.payload_len, 2);
        assert!(!transport.is_closed());
        let mut retry = first.receiver;
        retry.offset = 0;
        let second = transport
            .call_into(request(22), retry, &CallOptions::default())
            .await;
        assert!(second.result.is_ok());
        assert_eq!(second.receiver.bytes.as_ptr(), pointer);
        assert_eq!(second.receiver.bytes, b"ok");
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn receiver_errors_and_panics_return_owner_and_retire_reader_generation() {
    bounded(async {
        for panic in [false, true] {
            let (transport, mut peer) = transport(128);
            let mut destination = Destination::new(8);
            destination.fail_advance = !panic;
            destination.panic_advance = panic;
            let pointer = destination.bytes.as_ptr();
            let options = CallOptions::default();
            let mut call = Box::pin(transport.call_into(request(31), destination, &options));
            held(call.as_mut()).await;
            read_rpc_message(&mut peer).await.unwrap();
            let other = Destination::new(8);
            let other_pointer = other.bytes.as_ptr();
            let mut waiting = Box::pin(transport.call_into(request(32), other, &options));
            held(waiting.as_mut()).await;
            read_rpc_message(&mut peer).await.unwrap();
            write_rpc_message(&mut peer, &reply(31, b"bad"))
                .await
                .unwrap();
            let result = call.await;
            assert!(result.result.is_err());
            assert_eq!(result.receiver.bytes.as_ptr(), pointer);
            assert_eq!(result.receiver.offset, 3);
            assert_eq!(result.payload_len, 3);
            assert!(transport.is_closed());
            let other = waiting.await;
            assert!(other.result.is_err());
            assert_eq!(other.receiver.bytes.as_ptr(), other_pointer);
            let result = transport
                .call_into(request(33), result.receiver, &options)
                .await;
            assert!(result.result.is_err());
            assert_eq!(result.receiver.bytes.as_ptr(), pointer);
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn reply_deadline_returns_quiescent_owner_for_reuse_on_same_connection() {
    let (transport, mut peer) = transport(128);
    let (seen, observed) = oneshot::channel();
    let mut destination = Destination::new(8);
    destination.on_buffer = Some(seen);
    let pointer = destination.bytes.as_ptr();
    let options = CallOptions::default().with_timeout(Duration::from_secs(5));
    let mut call = Box::pin(transport.call_into(request(41), destination, &options));
    held(call.as_mut()).await;
    read_rpc_message(&mut peer).await.unwrap();
    let encoded = reply(41, b"abcdefgh").encode().unwrap();
    // The reader owns the destination and is waiting for the advertised data.
    peer.write_all(&(0x8000_0000 | encoded.len() as u32).to_be_bytes())
        .await
        .unwrap();
    peer.write_all(&encoded[..encoded.len() - 8]).await.unwrap();
    observed.await.unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    let first = call.await;
    assert_eq!(
        first.result.unwrap_err(),
        RuntimeError::CallTimeout {
            duration: Duration::from_secs(5)
        }
    );
    assert_eq!(first.receiver.bytes.as_ptr(), pointer);
    assert_eq!(first.payload_len, 0);
    assert!(!transport.is_closed());
    let mut receiver = first.receiver;
    receiver.bytes.fill(0x7a);
    // The late body must drain into scratch, even after the same allocation has
    // been returned to the caller and reused by another call on this connection.
    peer.write_all(b"abcdefgh").await.unwrap();
    let server = tokio::spawn(async move {
        assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(42));
        write_rpc_message(&mut peer, &reply(42, b"retry"))
            .await
            .unwrap();
    });
    let second = transport
        .call_into(request(42), receiver, &CallOptions::default())
        .await;
    assert!(second.result.is_ok());
    assert_eq!(second.receiver.bytes.as_ptr(), pointer);
    assert_eq!(&second.receiver.bytes[..5], b"retry");
    assert_eq!(&second.receiver.bytes[5..], &[0x7a; 3]);

    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unsent_deadline_recovers_owner_without_retiring_another_writer() {
    let (transport, mut peer) = transport(1);
    let options = CallOptions::default();
    let mut active = Box::pin(transport.call_into(request(51), Destination::new(8), &options));
    held(active.as_mut()).await;
    let destination = Destination::new(8);
    let pointer = destination.bytes.as_ptr();
    let short = CallOptions::default().with_timeout(Duration::from_secs(1));
    let mut queued = Box::pin(transport.call_into(request(52), destination, &short));
    held(queued.as_mut()).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let outcome = queued.await;
    assert_eq!(
        outcome.result.unwrap_err(),
        RuntimeError::CallTimeout {
            duration: Duration::from_secs(1)
        }
    );
    assert_eq!(outcome.receiver.bytes.as_ptr(), pointer);
    assert!(!transport.is_closed());
    transport.close();
    let outcome = active.await;
    assert!(outcome.result.is_err());
    let mut byte = [0];
    use tokio::io::AsyncReadExt;
    assert_eq!(peer.read(&mut byte).await.unwrap(), 1);
    assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
}

#[tokio::test]
async fn explicit_close_returns_active_destination_after_reader_cleanup() {
    bounded(async {
        let (transport, mut peer) = transport(128);
        let (seen, observed) = oneshot::channel();
        let mut destination = Destination::new(8);
        destination.on_buffer = Some(seen);
        let pointer = destination.bytes.as_ptr();
        let options = CallOptions::default();
        let mut call = Box::pin(transport.call_into(request(61), destination, &options));
        held(call.as_mut()).await;
        read_rpc_message(&mut peer).await.unwrap();
        let encoded = reply(61, b"abcdefgh").encode().unwrap();
        peer.write_all(&(0x8000_0000 | encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        peer.write_all(&encoded[..encoded.len() - 8]).await.unwrap();
        observed.await.unwrap();
        transport.close();
        let result = call.await;
        assert!(result.result.is_err());
        assert_eq!(result.receiver.bytes.as_ptr(), pointer);
        assert!(peer.write_all(b"abcdefgh").await.is_err());
    })
    .await;
}

// Representative NFS READ result placement: parse a variable small prefix,
// receive data into the current admitted allocation's tail, and consume XDR pad.
#[derive(Debug)]
struct PrefixReceiver {
    data: Vec<u8>,
    completed: usize,
    header: [u8; 104],
    header_read: usize,
    header_len: usize,
    count: usize,
    data_read: usize,
    padding: [u8; 3],
    padding_read: usize,
}
impl PrefixReceiver {
    fn new(length: usize) -> Self {
        Self {
            data: vec![0; length],
            completed: 0,
            header: [0; 104],
            header_read: 0,
            header_len: 8,
            count: 0,
            data_read: 0,
            padding: [0; 3],
            padding_read: 0,
        }
    }
    fn next_read(mut self) -> Self {
        self.completed += self.data_read;
        self.header_read = 0;
        self.header_len = 8;
        self.count = 0;
        self.data_read = 0;
        self.padding_read = 0;
        self
    }
}
impl ReplyReceiver for PrefixReceiver {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        if self.header_read < self.header_len {
            return Ok(&mut self.header[self.header_read..self.header_len]);
        }
        if self.data_read < self.count {
            return Ok(&mut self.data[self.completed + self.data_read..self.completed + self.count]);
        }
        let pad = (4 - self.count % 4) % 4;
        Ok(&mut self.padding[self.padding_read..pad])
    }
    fn advance(&mut self, bytes: usize) -> Result<(), RuntimeError> {
        if self.header_read < self.header_len {
            self.header_read += bytes;
            if self.header_read == 8 {
                assert_eq!(&self.header[..4], &[0; 4]);
                self.header_len = match u32::from_be_bytes(self.header[4..8].try_into().unwrap()) {
                    0 => 20,
                    1 => 104,
                    _ => panic!("invalid post_op_attr discriminator"),
                };
            }
            if self.header_read == self.header_len {
                let suffix = self.header_len - 12;
                self.count = u32::from_be_bytes(self.header[suffix..suffix + 4].try_into().unwrap())
                    as usize;
                assert_eq!(
                    self.count,
                    u32::from_be_bytes(self.header[suffix + 8..suffix + 12].try_into().unwrap())
                        as usize
                );
                assert!(self.completed + self.count <= self.data.len());
            }
        } else if self.data_read < self.count {
            self.data_read += bytes;
        } else {
            self.padding_read += bytes;
        }
        Ok(())
    }
}
fn read_result(data: &[u8], attributes: bool) -> Vec<u8> {
    let mut result = vec![0; if attributes { 104 } else { 20 }];
    result[4..8].copy_from_slice(&u32::from(attributes).to_be_bytes());
    if attributes {
        result[8..12].copy_from_slice(&1u32.to_be_bytes());
    }
    let suffix = result.len() - 12;
    result[suffix..suffix + 4].copy_from_slice(&(data.len() as u32).to_be_bytes());
    result[suffix + 8..suffix + 12].copy_from_slice(&(data.len() as u32).to_be_bytes());
    result.extend_from_slice(data);
    result.resize((result.len() + 3) & !3, 0);
    result
}

#[tokio::test]
async fn variable_prefix_short_reads_append_into_same_owned_allocation_without_copy() {
    bounded(async {
        let (transport, mut peer) = transport(3);
        let server = tokio::spawn(async move {
            for (xid, payload, attributes) in [
                (71, b"abc".as_slice(), false),
                (72, b"defgh".as_slice(), true),
            ] {
                assert_eq!(read_rpc_message(&mut peer).await.unwrap().xid, Xid(xid));
                let encoded = reply(xid, &read_result(payload, attributes))
                    .encode()
                    .unwrap();
                for fragment in fragment_record(&encoded, 3).unwrap() {
                    peer.write_all(&fragment).await.unwrap();
                }
            }
        });
        let destination = PrefixReceiver::new(8);
        let pointer = destination.data.as_ptr();
        let first = transport
            .call_into(request(71), destination, &CallOptions::default())
            .await;
        assert!(first.result.is_ok());
        assert_eq!(first.payload_len, 24);
        assert_eq!(first.receiver.data_read, 3);
        assert_eq!(&first.receiver.data[..3], b"abc");
        assert_eq!(first.receiver.data.as_ptr(), pointer);
        let second = transport
            .call_into(
                request(72),
                first.receiver.next_read(),
                &CallOptions::default(),
            )
            .await;
        assert!(second.result.is_ok());
        assert_eq!(second.payload_len, 112);
        assert_eq!(second.receiver.data, b"abcdefgh");
        assert_eq!(second.receiver.data.as_ptr(), pointer);
        assert_eq!(second.receiver.completed + second.receiver.data_read, 8);
        server.await.unwrap();
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn partial_frame_write_deadline_returns_original_owner_after_retirement() {
    let (stream, mut peer) = duplex(1);
    let mut config = ClientConfig::new("127.0.0.1:0".parse().unwrap());
    config.write_timeout = Some(Duration::from_secs(1));
    let transport = OwnedRpcTransport::from_stream(stream, &config).unwrap();
    let destination = Destination::new(8);
    let pointer = destination.bytes.as_ptr();
    let options = CallOptions::default();
    let mut call = Box::pin(transport.call_into(request(81), destination, &options));
    held(call.as_mut()).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let result = call.await;
    assert_eq!(
        result.result.unwrap_err(),
        RuntimeError::WriteTimeout {
            duration: Duration::from_secs(1)
        }
    );
    assert_eq!(result.receiver.bytes.as_ptr(), pointer);
    assert_eq!(result.payload_len, 0);
    assert!(transport.is_closed());
    let mut partial = Vec::new();
    use tokio::io::AsyncReadExt;
    peer.read_to_end(&mut partial).await.unwrap();
    assert_eq!(partial.len(), 1);
}
