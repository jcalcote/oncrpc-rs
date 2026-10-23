//! Run with `cargo run -p owned-buffers-example`.
//!
//! A local echo peer returns one XDR opaque value: a four-byte length followed
//! by data. The caller owns the receive allocation before sending the request.
//! Its receiver parses the small length field, then directs subsequent socket
//! reads into that allocation. No live server or filesystem is required.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AdmissionError, AdmissionLimits, CallOptions, ClientConfig,
    MessageBody, OpaqueAuth, OwnedRpcTransport, Procedure, ProgramVersion, RecordLimits, ReplyBody,
    ReplyReceiver, RpcMessage, RuntimeError, SegmentedRequest, Xid, read_rpc_message,
    write_rpc_message,
};
use onc_rpc_wire::CallBody;
use std::time::Duration;

#[derive(Debug)]
struct OpaqueReceiver {
    length: [u8; 4],
    length_used: usize,
    data: Vec<u8>,
    data_used: usize,
    expected: usize,
}
impl OpaqueReceiver {
    fn new(capacity: usize) -> Self {
        Self {
            length: [0; 4],
            length_used: 0,
            data: vec![0; capacity],
            data_used: 0,
            expected: 0,
        }
    }
}
impl ReplyReceiver for OpaqueReceiver {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        if self.length_used < 4 {
            Ok(&mut self.length[self.length_used..])
        } else {
            Ok(&mut self.data[self.data_used..self.expected])
        }
    }

    fn advance(&mut self, count: usize) -> Result<(), RuntimeError> {
        if self.length_used < 4 {
            self.length_used += count;
            if self.length_used == 4 {
                self.expected = usize::try_from(u32::from_be_bytes(self.length))
                    .map_err(|_| RuntimeError::Transport("length does not fit usize".into()))?;
                // This small example accepts four-byte-aligned values, so no
                // separate XDR padding destination is needed.
                if self.expected > self.data.len() || self.expected % 4 != 0 {
                    return Err(RuntimeError::Transport(
                        "invalid opaque value length".into(),
                    ));
                }
            }
        } else {
            self.data_used += count;
        }
        Ok(())
    }
}

fn request(xid: u32, data: Bytes) -> SegmentedRequest {
    SegmentedRequest {
        header: RpcMessage {
            xid: Xid(xid), // Every call on this connection uses a fresh XID.
            body: MessageBody::Call(CallBody::new(
                ProgramVersion {
                    program: 0x3123_0001,
                    version: 1,
                },
                Procedure(1),
                OpaqueAuth::none(),
                OpaqueAuth::none(),
                Bytes::new(),
            )),
        },
        // Only the small length field is copied. The data stays in its owner.
        payload: vec![Bytes::copy_from_slice(&8_u32.to_be_bytes()), data],
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), run()).await??;
    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // A tiny duplex capacity deliberately exercises partial reads and writes.
    let (stream, mut peer) = tokio::io::duplex(3);
    let config = ClientConfig::new("127.0.0.1:0".parse()?);
    let transport = OwnedRpcTransport::from_stream_with_admission(
        stream,
        &config,
        RecordLimits::default(),
        AdmissionLimits {
            max_calls: 1,
            max_bytes: 4096,
        },
    )?;
    let server = tokio::spawn(async move {
        let call = read_rpc_message(&mut peer).await?;
        let MessageBody::Call(body) = call.body else {
            return Err(RuntimeError::UnexpectedCallMessage);
        };
        // The toy peer echoes the XDR arguments as results. Its conventional
        // message API is independent of the caller-owned receive path.
        let reply = RpcMessage {
            xid: call.xid,
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(body.payload),
            })),
        };
        write_rpc_message(&mut peer, &reply).await?;
        read_rpc_message(&mut peer).await?;
        // Deliberately close instead of answering the second call.
        Ok::<(), RuntimeError>(())
    });

    let bytes = b"ownedRPC".to_vec();
    let original_send = bytes.as_ptr();
    let data = Bytes::from_owner(bytes);
    assert_eq!(data.as_ptr(), original_send);
    let receiver = OpaqueReceiver::new(16);
    let original_receive = receiver.data.as_ptr();
    // Admission is immediate, before either future is polled. A full connection
    // returns both owners; it does not enqueue another payload or close the peer.
    let first =
        transport.try_call_into(request(1, data.clone()), receiver, &CallOptions::default())?;
    let refused = match transport.try_call_into(
        request(2, data.clone()),
        OpaqueReceiver::new(16),
        &CallOptions::default(),
    ) {
        Err(refused) => refused,
        Ok(_) => return Err("second call should exceed the one-call limit".into()),
    };
    assert_eq!(refused.reason, AdmissionError::AtCapacity);
    assert!(!transport.is_closed());
    println!("PASS: admission refused an unsent call and returned both owners");
    let outcome = first.await;
    let header = outcome.result?;
    assert!(matches!(
        header.body,
        MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            status: AcceptedStatus::Success(_),
            ..
        }))
    ));
    let receiver = outcome.receiver;
    assert_eq!(receiver.data.as_ptr(), original_receive);
    assert_eq!(receiver.data_used, receiver.expected);
    assert_eq!(outcome.payload_len, 12); // Four XDR length bytes plus eight data bytes.
    assert_eq!(&receiver.data[..receiver.data_used], b"ownedRPC");
    println!(
        "PASS: received '{}' in the original caller allocation",
        std::str::from_utf8(&receiver.data[..receiver.data_used])?
    );

    // Retry the exact refused owners once capacity is available. The toy peer
    // closes on this call, demonstrating quiescent ownership return on failure.
    let original_receive = refused.receiver.data.as_ptr();
    transport.wait_for_capacity(refused.required_bytes).await?;
    let outcome = transport
        .try_call_into(refused.request, refused.receiver, &CallOptions::default())?
        .await;
    assert!(outcome.result.is_err());
    assert_eq!(outcome.receiver.data.as_ptr(), original_receive);
    println!("PASS: peer closure returned the original receiver safely");
    server.await??;
    Ok(())
}
