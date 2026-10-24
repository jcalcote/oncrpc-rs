//! Exercise generated methods against the public RFC 1813 NFSv3 I/O layouts.
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AdmissionError, AdmissionLimits, AsyncClient, CallOptions,
    ClientConfig, MessageBody, OpaqueAuth, OwnedRpcTransport, Procedure, RecordLimits, ReplyBody,
    RpcMessage, Xid, fragment_record, read_rpc_message, write_rpc_message,
};
use onc_rpc_xdr::{OpaqueSegments, XdrDecode, XdrEncode, XdrEncodeSegments, XdrReplyMetadata};
use onc_rpcgen::generate_from_x_file;
use std::{future::Future, path::Path, time::Duration};
use tokio::io::{AsyncWriteExt, DuplexStream, duplex};

#[allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::module_inception,
    clippy::upper_case_acronyms
)]
mod nfs3_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/nfs3_io.generated.types.rs.txt"
    ));
}
#[allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::module_inception,
    clippy::upper_case_acronyms
)]
mod nfs3_io_stubs {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/nfs3_io.generated.stubs.rs.txt"
    ));
}
use nfs3_io::*;
type Stub = nfs3_io_stubs::nfs_program::nfs_v3::async_client::NFS_V3Client<OwnedRpcTransport>;

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("generated NFS operation exceeded test deadline")
}

fn connected() -> (Stub, OwnedRpcTransport, DuplexStream) {
    let (stream, peer) = duplex(512);
    let config = ClientConfig::new("127.0.0.1:2049".parse().unwrap());
    let transport = OwnedRpcTransport::from_stream_with_admission(
        stream,
        &config,
        RecordLimits::default(),
        AdmissionLimits {
            max_calls: 1,
            max_bytes: 128 * 1024,
        },
    )
    .unwrap();
    let stub = Stub::new(AsyncClient::new(config, transport.clone()));
    (stub, transport, peer)
}

fn attributes() -> fattr3 {
    fattr3 {
        r#type: ftype3::NF3REG,
        mode: 0o640,
        nlink: 2,
        uid: 1000,
        gid: 100,
        size: (1 << 40) + 11,
        used: 4096,
        rdev: specdata3 {
            specdata1: 0,
            specdata2: 0,
        },
        fsid: 123,
        fileid: (1 << 35) + 9,
        atime: nfstime3 {
            seconds: 1,
            nseconds: 2,
        },
        mtime: nfstime3 {
            seconds: 3,
            nseconds: 4,
        },
        ctime: nfstime3 {
            seconds: 5,
            nseconds: 6,
        },
    }
}

fn post_attributes(present: bool) -> post_op_attr {
    if present {
        post_op_attr::True {
            attributes: attributes(),
        }
    } else {
        post_op_attr::False
    }
}

fn wcc() -> wcc_data {
    wcc_data {
        before: pre_op_attr::True {
            attributes: wcc_attr {
                size: 9,
                mtime: nfstime3 {
                    seconds: 3,
                    nseconds: 4,
                },
                ctime: nfstime3 {
                    seconds: 5,
                    nseconds: 6,
                },
            },
        },
        after: post_attributes(true),
    }
}

fn read_args(count: usize) -> READ3args {
    READ3args {
        file: nfs_fh3 {
            data: Bytes::from_static(b"fh!"),
        },
        offset: 0x0102_0304_0506_0708,
        count: count as u32,
    }
}

fn success(xid: Xid, payload: Bytes) -> RpcMessage {
    RpcMessage {
        xid,
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::none(),
            status: AcceptedStatus::Success(payload),
        })),
    }
}

async fn fragmented_reply(peer: &mut DuplexStream, reply: RpcMessage) {
    // A 97-byte fragment boundary cuts through the longest READ control prefix.
    for fragment in fragment_record(&reply.encode().unwrap(), 97).unwrap() {
        peer.write_all(&fragment).await.unwrap();
    }
}

#[test]
fn generated_nfs_snapshots_remain_reproducible() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/xdr/real/nfs3_io.x");
    let output = generate_from_x_file(fixture).unwrap();
    let module = output
        .modules
        .iter()
        .find(|m| m.module_name == "nfs3_io")
        .unwrap();
    assert_eq!(
        module.types.as_deref().unwrap(),
        include_str!("../../../tests/fixtures/expected/nfs3_io.generated.types.rs.txt")
    );
    assert_eq!(
        module.stubs.as_deref().unwrap(),
        include_str!("../../../tests/fixtures/expected/nfs3_io.generated.stubs.rs.txt")
    );
    assert_eq!(READ3resMetadata::MAX_PREFIX_BYTES, 104);
}

#[test]
fn generated_write_segments_match_standard_wire_and_retain_payload() {
    let data = Bytes::from_owner(b"hello".to_vec());
    let original = data.as_ptr();
    let request = WRITE3args {
        file: nfs_fh3 {
            data: Bytes::from_static(b"fh!"),
        },
        offset: 0x0102_0304_0506_0708,
        count: 5,
        stable: stable_how::UNSTABLE,
        data,
    };
    let segments = request.to_xdr_segments().unwrap();
    assert!(
        segments
            .iter()
            .any(|part| part.as_ptr() == original && part.len() == 5)
    );
    let wire: Vec<u8> = segments
        .iter()
        .flat_map(|part| part.iter().copied())
        .collect();
    assert_eq!(wire, request.to_xdr_bytes().unwrap());
    // RFC 1813 WRITE fields: handle, 64-bit offset, count, stability, opaque data.
    assert_eq!(
        wire,
        [
            0, 0, 0, 3, b'f', b'h', b'!', 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0,
            0, 5, b'h', b'e', b'l', b'l', b'o', 0, 0, 0,
        ]
    );
    assert_eq!(WRITE3args::from_xdr_bytes(&wire).unwrap(), request);

    let first = Bytes::from_owner(b"he".to_vec());
    let second = Bytes::from_owner(b"llo".to_vec());
    let first_address = first.as_ptr();
    let second_address = second.as_ptr();
    let mut split: WRITE3argsSegments = request.into();
    split.data = OpaqueSegments::try_new(vec![first, second]).unwrap();
    let segments = split.to_xdr_segments().unwrap();
    assert!(
        segments
            .iter()
            .any(|part| part.as_ptr() == first_address && part.len() == 2)
    );
    assert!(
        segments
            .iter()
            .any(|part| part.as_ptr() == second_address && part.len() == 3)
    );
    let split_wire: Vec<u8> = segments
        .iter()
        .flat_map(|part| part.iter().copied())
        .collect();
    // Multiple owners contribute to one length and one final padding sequence.
    assert_eq!(split_wire, wire);
}

#[tokio::test]
async fn generated_read_receives_all_attribute_and_padding_shapes() {
    bounded(async {
        let (stub, _transport, mut peer) = connected();
        let cases: Vec<_> = [false, true]
            .into_iter()
            .flat_map(|attrs| {
                [0usize, 1, 2, 3, 8, 65_537]
                    .into_iter()
                    .map(move |length| (attrs, length))
            })
            .collect();
        let server_cases = cases.clone();
        let server = tokio::spawn(async move {
            for (present, length) in server_cases {
                let request = read_rpc_message(&mut peer).await.unwrap();
                let MessageBody::Call(call) = request.body else {
                    panic!("expected call")
                };
                assert_eq!(call.program.program, 100_003);
                assert_eq!(call.program.version, 3);
                assert_eq!(call.procedure, Procedure(6));
                assert_eq!(
                    READ3args::from_xdr_bytes(&call.payload).unwrap(),
                    read_args(length)
                );
                let reply = READ3res::Nfs3Ok {
                    resok: READ3resok {
                        file_attributes: post_attributes(present),
                        count: length as u32,
                        eof: true,
                        data: Bytes::from(vec![0xa5; length]),
                    },
                };
                fragmented_reply(
                    &mut peer,
                    success(request.xid, reply.to_xdr_bytes().unwrap()),
                )
                .await;
            }
        });
        for (present, length) in cases {
            let buffer = vec![0x55; length + 16];
            let original = buffer.as_ptr();
            let outcome = stub
                .nfsproc3_read_into(read_args(length), buffer, &CallOptions::default())
                .await;
            assert_eq!(outcome.buffer.as_ptr(), original);
            assert_eq!(outcome.payload_len, length);
            assert!(outcome.buffer[..length].iter().all(|byte| *byte == 0xa5));
            assert!(outcome.buffer[length..].iter().all(|byte| *byte == 0x55));
            let READ3resMetadata::Nfs3Ok { resok } = outcome.result.unwrap() else {
                panic!("read failed")
            };
            assert_eq!(resok.file_attributes, post_attributes(present));
            assert_eq!(resok.count, length as u32);
            assert!(resok.eof);
            assert_eq!(resok.data.len(), length);
        }
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn generated_read_returns_typed_remote_errors_without_touching_data() {
    bounded(async {
        let (stub, _transport, mut peer) = connected();
        let server = tokio::spawn(async move {
            for present in [false, true] {
                let request = read_rpc_message(&mut peer).await.unwrap();
                let reply = READ3res::Default {
                    discriminant: nfsstat3::NFS3ERR_STALE as i32,
                    resfail: READ3resfail {
                        file_attributes: post_attributes(present),
                    },
                };
                fragmented_reply(
                    &mut peer,
                    success(request.xid, reply.to_xdr_bytes().unwrap()),
                )
                .await;
            }
        });
        for present in [false, true] {
            let buffer = vec![0x55; 8];
            let original = buffer.as_ptr();
            let outcome = stub
                .nfsproc3_read_into(read_args(8), buffer, &CallOptions::default())
                .await;
            assert_eq!(outcome.buffer.as_ptr(), original);
            assert_eq!(outcome.buffer, [0x55; 8]);
            assert_eq!(outcome.payload_len, 0);
            let READ3resMetadata::Default {
                discriminant,
                resfail,
            } = outcome.result.unwrap()
            else {
                panic!("expected stale result")
            };
            assert_eq!(discriminant, nfsstat3::NFS3ERR_STALE as i32);
            assert_eq!(resfail.file_attributes, post_attributes(present));
        }
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn generated_write_and_commit_decode_durability_and_verifier() {
    bounded(async {
        let (stub, _transport, mut peer) = connected();
        let server = tokio::spawn(async move {
            let request = read_rpc_message(&mut peer).await.unwrap();
            let MessageBody::Call(call) = request.body else {
                panic!("expected call")
            };
            assert_eq!(call.procedure, Procedure(7));
            let args = WRITE3args::from_xdr_bytes(&call.payload).unwrap();
            assert_eq!(args.offset, 0x0102_0304_0506_0708);
            assert_eq!(args.data.as_ref(), b"hello");
            let reply = WRITE3res::Nfs3Ok {
                resok: WRITE3resok {
                    file_wcc: wcc(),
                    count: 3,
                    committed: stable_how::UNSTABLE,
                    verf: *b"verifier",
                },
            };
            write_rpc_message(
                &mut peer,
                &success(request.xid, reply.to_xdr_bytes().unwrap()),
            )
            .await
            .unwrap();
            let request = read_rpc_message(&mut peer).await.unwrap();
            let MessageBody::Call(call) = request.body else {
                panic!("expected call")
            };
            assert_eq!(call.procedure, Procedure(21));
            assert_eq!(COMMIT3args::from_xdr_bytes(&call.payload).unwrap().count, 3);
            let reply = COMMIT3res::Nfs3Ok {
                resok: COMMIT3resok {
                    file_wcc: wcc(),
                    verf: *b"verifier",
                },
            };
            write_rpc_message(
                &mut peer,
                &success(request.xid, reply.to_xdr_bytes().unwrap()),
            )
            .await
            .unwrap();
        });
        let args = WRITE3args {
            file: nfs_fh3 {
                data: Bytes::from_static(b"fh!"),
            },
            offset: 0x0102_0304_0506_0708,
            count: 5,
            stable: stable_how::UNSTABLE,
            data: Bytes::from_owner(b"hello".to_vec()),
        };
        let mut args: WRITE3argsSegments = args.into();
        args.data = OpaqueSegments::try_new(vec![
            Bytes::from_owner(b"he".to_vec()),
            Bytes::from_owner(b"llo".to_vec()),
        ])
        .unwrap();
        let reply_buffer = vec![0; 256];
        let original = reply_buffer.as_ptr();
        let outcome = stub
            .nfsproc3_write_segmented(args, reply_buffer, &CallOptions::default())
            .await;
        assert_eq!(outcome.buffer.as_ptr(), original);
        let WRITE3res::Nfs3Ok { resok } = outcome.result.unwrap() else {
            panic!("write failed")
        };
        assert_eq!(resok.count, 3);
        assert_eq!(resok.committed, stable_how::UNSTABLE);
        assert_eq!(resok.verf, *b"verifier");
        assert_eq!(resok.file_wcc, wcc());
        let commit = stub
            .nfsproc3_commit(COMMIT3args {
                file: nfs_fh3 {
                    data: Bytes::from_static(b"fh!"),
                },
                offset: 0x0102_0304_0506_0708,
                count: 3,
            })
            .await
            .unwrap();
        let COMMIT3res::Nfs3Ok { resok } = commit else {
            panic!("commit failed")
        };
        assert_eq!(resok.verf, *b"verifier");
        assert_eq!(resok.file_wcc, wcc());
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn generated_read_admission_refusal_returns_unsent_owner_for_retry() {
    bounded(async {
        let (stub, transport, mut peer) = connected();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let request = read_rpc_message(&mut peer).await.unwrap();
                let reply = READ3res::Nfs3Ok {
                    resok: READ3resok {
                        file_attributes: post_attributes(false),
                        count: 1,
                        eof: false,
                        data: Bytes::from_static(b"x"),
                    },
                };
                write_rpc_message(
                    &mut peer,
                    &success(request.xid, reply.to_xdr_bytes().unwrap()),
                )
                .await
                .unwrap();
            }
        });
        let first = stub
            .try_nfsproc3_read_into(read_args(1), vec![0; 1], &CallOptions::default())
            .unwrap();
        let buffer = vec![0; 1];
        let original = buffer.as_ptr();
        let refused =
            match stub.try_nfsproc3_read_into(read_args(1), buffer, &CallOptions::default()) {
                Err(refused) => refused,
                Ok(_) => panic!("one-call admission must refuse second unsent call"),
            };
        assert_eq!(refused.reason, AdmissionError::AtCapacity);
        assert!(!transport.is_closed());
        first.await.result.unwrap();
        let outcome = stub
            .try_nfsproc3_read_into(refused.request, refused.receiver, &CallOptions::default())
            .unwrap()
            .await;
        outcome.result.unwrap();
        assert_eq!(outcome.buffer.as_ptr(), original);
        assert_eq!(outcome.buffer, b"x");
        server.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn generated_read_recovers_buffer_on_malformed_or_lost_reply() {
    bounded(async {
        for payload in [None, Some(Bytes::from_static(&[0, 0, 0, 0, 0, 0, 0, 2]))] {
            let (stub, _transport, mut peer) = connected();
            let server = tokio::spawn(async move {
                let request = read_rpc_message(&mut peer).await.unwrap();
                if let Some(payload) = payload {
                    let _ = write_rpc_message(&mut peer, &success(request.xid, payload)).await;
                }
            });
            let buffer = vec![0x55; 8];
            let original = buffer.as_ptr();
            let outcome = stub
                .nfsproc3_read_into(read_args(8), buffer, &CallOptions::default())
                .await;
            assert!(outcome.result.is_err());
            assert_eq!(outcome.buffer.as_ptr(), original);
            assert_eq!(outcome.buffer, [0x55; 8]);
            server.await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn generated_read_rejects_excess_lengths_bad_padding_and_truncated_data() {
    bounded(async {
        let body = |data: &'static [u8]| {
            READ3res::Nfs3Ok {
                resok: READ3resok {
                    file_attributes: post_attributes(false),
                    count: data.len() as u32,
                    eof: false,
                    data: Bytes::from_static(data),
                },
            }
            .to_xdr_bytes()
            .unwrap()
            .to_vec()
        };
        let oversized = body(b"123456789");
        let mut bad_padding = body(b"x");
        *bad_padding.last_mut().unwrap() = 1;
        let mut truncated = body(b"hello");
        truncated.truncate(22); // Complete 20-byte metadata followed by two data bytes.
        for (payload, received) in [(oversized, 0), (bad_padding, 1), (truncated, 2)] {
            let (stub, _transport, mut peer) = connected();
            let server = tokio::spawn(async move {
                let request = read_rpc_message(&mut peer).await.unwrap();
                let _ =
                    write_rpc_message(&mut peer, &success(request.xid, Bytes::from(payload))).await;
            });
            let buffer = vec![0x55; 8];
            let original = buffer.as_ptr();
            let outcome = stub
                .nfsproc3_read_into(read_args(8), buffer, &CallOptions::default())
                .await;
            assert!(outcome.result.is_err());
            assert_eq!(outcome.buffer.as_ptr(), original);
            assert_eq!(outcome.payload_len, received);
            assert!(outcome.buffer[received..].iter().all(|byte| *byte == 0x55));
            server.await.unwrap();
        }
    })
    .await;
}
