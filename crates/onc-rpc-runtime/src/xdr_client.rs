//! Typed bridges used by generated caller-owned client methods.
use crate::{
    AdmissionError, AsyncClient, CallOptions, CallRefusal, CallRequest, OwnedRpcTransport,
    Procedure, ProgramVersion, ReceiveOutcome, ReplyReceiver, RuntimeError, SegmentedRequest,
    build_request_message_with_options, handle_reply,
};
use bytes::Bytes;
use onc_rpc_xdr::{XdrDecode, XdrEncodeSegments, XdrError, XdrReplyMetadata};
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};

pin_project! {
    // Mapping an already constructed future with `async move { call.await }`
    // stores the captured call and its moved awaitee in separate frame slots.
    // Structural projection polls the child in its single original slot.
    struct MapCall<F, Complete> {
        #[pin]
        call: F,
        complete: Option<Complete>,
    }
}
impl<F, Complete, Output> Future for MapCall<F, Complete>
where
    F: Future,
    Complete: FnOnce(F::Output) -> Output,
{
    type Output = Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Output> {
        let this = self.project();
        this.call.poll(cx).map(|result| {
            this.complete
                .take()
                .expect("completed call must not be polled again")(result)
        })
    }
}

/// A typed completion together with the original, now quiescent destination.
///
/// For an `*_into` call, the result is generated metadata and the leading
/// `payload_len` bytes of `buffer` hold its terminal opaque value. For a
/// `*_segmented` call, the result is the ordinary decoded reply and
/// `payload_len` counts its complete XDR representation in `buffer`.
#[derive(Debug)]
pub struct TypedReceiveOutcome<M, B> {
    /// Decoded result, or a local/RPC error. Application error unions remain typed values.
    pub result: Result<M, RuntimeError>,
    /// The original destination, including on admission, decode and transport errors.
    pub buffer: B,
    /// Bytes received into the destination, including partial data on an error.
    pub payload_len: usize,
}

/// Generated metadata parser followed by direct receive into caller storage.
///
/// Control scratch is limited by the generated `MAX_PREFIX_BYTES`. An unsent
/// admission refusal returns this receiver; `into_buffer` recovers its owner.
pub struct XdrReplyReceiver<M, B> {
    buffer: B,
    prefix: Vec<u8>,
    used: usize,
    target: usize,
    metadata: Option<M>,
    data_len: usize,
    data_used: usize,
    padding: [u8; 3],
    padding_used: usize,
}

impl<M: XdrReplyMetadata, B: AsMut<[u8]>> XdrReplyReceiver<M, B> {
    fn new(buffer: B) -> Self {
        Self {
            buffer,
            prefix: vec![0; M::MAX_PREFIX_BYTES],
            used: 0,
            target: 0,
            metadata: None,
            data_len: 0,
            data_used: 0,
            padding: [0; 3],
            padding_used: 0,
        }
    }

    /// Recover the original destination from an unsent refusal.
    pub fn into_buffer(self) -> B {
        self.buffer
    }

    fn parse_prefix(&mut self) -> Result<(), RuntimeError> {
        if self.metadata.is_some() || self.used < self.target {
            return Ok(());
        }
        let mut input = &self.prefix[..self.used];
        match M::decode_xdr(&mut input) {
            Ok(metadata) => {
                if !input.is_empty() {
                    return Err(RuntimeError::Decode(XdrError::TrailingBytes(input.len())));
                }
                self.data_len = metadata.opaque_len().unwrap_or(0);
                if self.data_len > self.buffer.as_mut().len() {
                    return Err(RuntimeError::Transport(
                        "XDR opaque result exceeds caller destination".into(),
                    ));
                }
                self.metadata = Some(metadata);
                Ok(())
            }
            Err(XdrError::UnexpectedEof { needed }) => {
                // The decoder leaves the incomplete field unconsumed. Request
                // only its missing bytes, never bytes from the following data.
                let missing = needed.saturating_sub(input.len()).max(1);
                self.target = self
                    .used
                    .checked_add(missing)
                    .ok_or_else(|| RuntimeError::Transport("XDR prefix length overflow".into()))?;
                if self.target > self.prefix.len() {
                    return Err(RuntimeError::Transport(
                        "XDR metadata exceeds generated prefix bound".into(),
                    ));
                }
                Ok(())
            }
            Err(error) => Err(RuntimeError::Decode(error)),
        }
    }

    fn finish(mut self, result: Result<(), RuntimeError>) -> TypedReceiveOutcome<M, B> {
        let result = result.and_then(|()| {
            self.parse_prefix()?;
            if self.data_used != self.data_len || self.padding_used != (4 - self.data_len % 4) % 4 {
                return Err(RuntimeError::Transport(
                    "truncated XDR opaque result".into(),
                ));
            }
            self.metadata
                .take()
                .ok_or_else(|| RuntimeError::Transport("truncated XDR result metadata".into()))
        });
        TypedReceiveOutcome {
            result,
            buffer: self.buffer,
            payload_len: self.data_used,
        }
    }
}

impl<M, B> ReplyReceiver for XdrReplyReceiver<M, B>
where
    M: XdrReplyMetadata,
    B: AsMut<[u8]> + Send + 'static,
{
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        self.parse_prefix()?;
        if self.metadata.is_none() {
            Ok(&mut self.prefix[self.used..self.target])
        } else if self.data_used < self.data_len {
            Ok(&mut self.buffer.as_mut()[self.data_used..self.data_len])
        } else {
            let padding = (4 - self.data_len % 4) % 4;
            Ok(&mut self.padding[self.padding_used..padding])
        }
    }

    fn advance(&mut self, count: usize) -> Result<(), RuntimeError> {
        if self.metadata.is_none() {
            self.used += count;
            self.parse_prefix()
        } else if self.data_used < self.data_len {
            self.data_used += count;
            Ok(())
        } else {
            self.padding_used += count;
            if self.padding[..self.padding_used].iter().any(|b| *b != 0) {
                return Err(RuntimeError::Transport("nonzero XDR opaque padding".into()));
            }
            Ok(())
        }
    }
}

/// Caller-limited storage for an ordinary typed reply to a segmented request.
/// Returned intact by an unsent admission refusal.
pub struct XdrDecodeReceiver<Ret, B> {
    buffer: B,
    used: usize,
    result_type: std::marker::PhantomData<fn() -> Ret>,
}
impl<Ret: XdrDecode + 'static, B: AsMut<[u8]>> XdrDecodeReceiver<Ret, B> {
    fn new(buffer: B) -> Self {
        Self {
            buffer,
            used: 0,
            result_type: std::marker::PhantomData,
        }
    }
    /// Recover the original destination from an unsent refusal.
    pub fn into_buffer(self) -> B {
        self.buffer
    }
    fn finish(mut self, result: Result<(), RuntimeError>) -> TypedReceiveOutcome<Ret, B> {
        let result = result.and_then(|()| {
            Ret::from_xdr_bytes(&self.buffer.as_mut()[..self.used]).map_err(RuntimeError::Decode)
        });
        TypedReceiveOutcome {
            result,
            buffer: self.buffer,
            payload_len: self.used,
        }
    }
}
impl<Ret: 'static, B: AsMut<[u8]> + Send + 'static> ReplyReceiver for XdrDecodeReceiver<Ret, B> {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        Ok(&mut self.buffer.as_mut()[self.used..])
    }
    fn advance(&mut self, count: usize) -> Result<(), RuntimeError> {
        self.used += count;
        Ok(())
    }
}

impl AsyncClient<OwnedRpcTransport> {
    fn segmented_request(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        payload: Vec<Bytes>,
        options: &CallOptions,
    ) -> Result<SegmentedRequest, RuntimeError> {
        let xid = self.next_xid.allocate()?;
        Ok(SegmentedRequest {
            header: build_request_message_with_options(
                &self.config,
                xid,
                CallRequest::new(program, procedure, Bytes::new()),
                options,
            ),
            payload,
        })
    }

    /// Receive an IDL-defined terminal opaque value without staging its data.
    /// Generated methods select `M`; callers supply only typed arguments and storage.
    pub async fn call_xdr_into<Arg, M, B>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: &Arg,
        buffer: B,
        options: &CallOptions,
    ) -> TypedReceiveOutcome<M, B>
    where
        Arg: XdrEncodeSegments,
        M: XdrReplyMetadata,
        B: AsMut<[u8]> + Send + 'static,
    {
        // End the admission Result's lifetime before awaiting its accepted call.
        let call = match self.try_call_xdr_into(program, procedure, argument, buffer, options) {
            Ok(call) => call,
            Err(refusal) => {
                return TypedReceiveOutcome {
                    result: Err(self.transport.admission_error(refusal.reason)),
                    buffer: refusal.receiver,
                    payload_len: 0,
                };
            }
        };
        call.await
    }

    /// Immediately admit a generated receive operation, or return its unsent owners.
    /// The accepted future owns its state and starts I/O when polled.
    #[allow(
        clippy::result_large_err,
        reason = "Refusal returns original owners inline"
    )]
    pub fn try_call_xdr_into<Arg, M, B>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: Arg,
        buffer: B,
        options: &CallOptions,
    ) -> Result<
        impl Future<Output = TypedReceiveOutcome<M, B>> + Send + 'static + use<Arg, M, B>,
        CallRefusal<Arg, B>,
    >
    where
        Arg: XdrEncodeSegments,
        M: XdrReplyMetadata,
        B: AsMut<[u8]> + Send + 'static,
    {
        let receiver = XdrReplyReceiver::new(buffer);
        let request = argument
            .to_xdr_segments()
            .map_err(RuntimeError::Encode)
            .and_then(|payload| self.segmented_request(program, procedure, payload, options));
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                return Err(CallRefusal {
                    reason: AdmissionError::InvalidRequest(error),
                    request: argument,
                    receiver: receiver.into_buffer(),
                    required_bytes: 0,
                });
            }
        };
        self.transport
            .try_xdr_receive(request, receiver, options)
            .map_err(|refusal| CallRefusal {
                reason: refusal.reason,
                request: argument,
                receiver: refusal.receiver.into_buffer(),
                required_bytes: refusal.required_bytes,
            })
    }

    /// Send generated XDR segments and decode a reply in caller-limited storage.
    /// The request's opaque `Bytes` owners are retained, without copying their data.
    pub async fn call_xdr_segmented<Arg, Ret, B>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: &Arg,
        buffer: B,
        options: &CallOptions,
    ) -> TypedReceiveOutcome<Ret, B>
    where
        Arg: XdrEncodeSegments,
        Ret: XdrDecode + 'static,
        B: AsMut<[u8]> + Send + 'static,
    {
        let call = match self.try_call_xdr_segmented(program, procedure, argument, buffer, options)
        {
            Ok(call) => call,
            Err(refusal) => {
                return TypedReceiveOutcome {
                    result: Err(self.transport.admission_error(refusal.reason)),
                    buffer: refusal.receiver,
                    payload_len: 0,
                };
            }
        };
        call.await
    }

    /// Immediately admit a segmented typed call, or return all unsent owners.
    #[allow(
        clippy::result_large_err,
        reason = "Refusal returns original owners inline"
    )]
    pub fn try_call_xdr_segmented<Arg, Ret, B>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: Arg,
        buffer: B,
        options: &CallOptions,
    ) -> Result<
        impl Future<Output = TypedReceiveOutcome<Ret, B>> + Send + 'static + use<Arg, Ret, B>,
        CallRefusal<Arg, B>,
    >
    where
        Arg: XdrEncodeSegments,
        Ret: XdrDecode + 'static,
        B: AsMut<[u8]> + Send + 'static,
    {
        let receiver = XdrDecodeReceiver::new(buffer);
        let request = argument
            .to_xdr_segments()
            .map_err(RuntimeError::Encode)
            .and_then(|payload| self.segmented_request(program, procedure, payload, options));
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                return Err(CallRefusal {
                    reason: AdmissionError::InvalidRequest(error),
                    request: argument,
                    receiver: receiver.into_buffer(),
                    required_bytes: 0,
                });
            }
        };
        self.transport
            .try_xdr_decode(request, receiver, options)
            .map_err(|refusal| CallRefusal {
                reason: refusal.reason,
                request: argument,
                receiver: refusal.receiver.into_buffer(),
                required_bytes: refusal.required_bytes,
            })
    }
}

impl OwnedRpcTransport {
    /// Retry a refused segmented typed call using its original encoded owners.
    #[allow(
        clippy::result_large_err,
        reason = "Refusal returns original owners inline"
    )]
    pub fn try_xdr_decode<Ret, B>(
        &self,
        request: SegmentedRequest,
        receiver: XdrDecodeReceiver<Ret, B>,
        options: &CallOptions,
    ) -> Result<
        impl Future<Output = TypedReceiveOutcome<Ret, B>> + Send + 'static + use<Ret, B>,
        CallRefusal<SegmentedRequest, XdrDecodeReceiver<Ret, B>>,
    >
    where
        Ret: XdrDecode + 'static,
        B: AsMut<[u8]> + Send + 'static,
    {
        let xid = request.header.xid;
        let call = self.try_call_into(request, receiver, options)?;
        Ok(MapCall {
            call,
            complete: Some(
                move |ReceiveOutcome {
                          result, receiver, ..
                      }: ReceiveOutcome<XdrDecodeReceiver<Ret, B>>| {
                    receiver.finish(
                        result
                            .and_then(|header| handle_reply(xid, header))
                            .map(|_| ()),
                    )
                },
            ),
        })
    }

    /// Retry a refused generated receive using its already encoded request and owner.
    /// This also preserves per-request credentials encoded before the refusal.
    #[allow(
        clippy::result_large_err,
        reason = "Refusal returns original owners inline"
    )]
    pub fn try_xdr_receive<M, B>(
        &self,
        request: SegmentedRequest,
        receiver: XdrReplyReceiver<M, B>,
        options: &CallOptions,
    ) -> Result<
        impl Future<Output = TypedReceiveOutcome<M, B>> + Send + 'static + use<M, B>,
        CallRefusal<SegmentedRequest, XdrReplyReceiver<M, B>>,
    >
    where
        M: XdrReplyMetadata,
        B: AsMut<[u8]> + Send + 'static,
    {
        let xid = request.header.xid;
        let call = self.try_call_into(request, receiver, options)?;
        Ok(MapCall {
            call,
            complete: Some(
                move |ReceiveOutcome {
                          result, receiver, ..
                      }: ReceiveOutcome<XdrReplyReceiver<M, B>>| {
                    receiver.finish(
                        result
                            .and_then(|header| handle_reply(xid, header))
                            .map(|_| ()),
                    )
                },
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AcceptedReply, AcceptedStatus, AdmissionLimits, ClientConfig, MessageBody, OpaqueAuth,
        RecordLimits, ReplyBody, RpcMessage, read_rpc_message, write_rpc_message,
    };
    use tokio::io::duplex;

    #[derive(Debug, PartialEq)]
    struct Metadata(u32);
    impl XdrDecode for Metadata {
        fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
            u32::decode_xdr(input).map(Self)
        }
    }
    impl XdrReplyMetadata for Metadata {
        const MAX_PREFIX_BYTES: usize = 4;
        fn opaque_len(&self) -> Option<usize> {
            Some(self.0 as usize)
        }
    }

    #[tokio::test]
    async fn typed_bridges_store_each_pending_transport_future_once() {
        let (local, _peer) = duplex(512);
        let config = ClientConfig::new("127.0.0.1:2049".parse().unwrap());
        let transport = OwnedRpcTransport::from_stream(local, &config).unwrap();
        let client = AsyncClient::new(config, transport.clone());
        let program = ProgramVersion {
            program: 42,
            version: 1,
        };
        let options = CallOptions::default();
        let request = || {
            client
                .segmented_request(program, Procedure(1), vec![], &options)
                .unwrap()
        };

        let raw_read = transport
            .try_call_into(
                request(),
                XdrReplyReceiver::<Metadata, _>::new([0; 128]),
                &options,
            )
            .unwrap();
        let mapped_read = transport
            .try_xdr_receive(
                request(),
                XdrReplyReceiver::<Metadata, _>::new([0; 128]),
                &options,
            )
            .unwrap();
        let raw_decode = transport
            .try_call_into(
                request(),
                XdrDecodeReceiver::<u32, _>::new([0; 128]),
                &options,
            )
            .unwrap();
        let mapped_decode = transport
            .try_xdr_decode(
                request(),
                XdrDecodeReceiver::<u32, _>::new([0; 128]),
                &options,
            )
            .unwrap();

        // Pointer-scaled slack allows layout/alignment changes across targets.
        // Mapping may add completion state, but never a second child frame.
        let mapping_slack = 4 * std::mem::size_of::<usize>();
        assert!(
            std::mem::size_of_val(&mapped_read) <= std::mem::size_of_val(&raw_read) + mapping_slack
        );
        assert!(
            std::mem::size_of_val(&mapped_decode)
                <= std::mem::size_of_val(&raw_decode) + mapping_slack
        );

        let read = client.call_xdr_into::<_, Metadata, _>(
            program,
            Procedure(1),
            &1u32,
            [0; 128],
            &options,
        );
        let decode = client.call_xdr_segmented::<_, u32, _>(
            program,
            Procedure(1),
            &1u32,
            [0; 128],
            &options,
        );
        // The async convenience retains its inputs and one mapped call. Its
        // admission Result must not retain another complete mapped call.
        let input_slack = std::mem::size_of::<[u8; 128]>() + 32 * std::mem::size_of::<usize>();
        assert!(std::mem::size_of_val(&read) <= std::mem::size_of_val(&mapped_read) + input_slack);
        assert!(
            std::mem::size_of_val(&decode) <= std::mem::size_of_val(&mapped_decode) + input_slack
        );
    }

    fn feed(receiver: &mut impl ReplyReceiver, bytes: &[u8]) -> Result<(), RuntimeError> {
        for byte in bytes {
            let output = receiver.buffer()?;
            assert!(!output.is_empty(), "receiver unexpectedly full");
            output[0] = *byte;
            receiver.advance(1)?;
        }
        Ok(())
    }

    #[test]
    fn direct_receiver_handles_short_reads_empty_data_and_padding() {
        for data in [b"".as_slice(), b"x", b"hello", b"12345678"] {
            let buffer = vec![0; 16];
            let original = buffer.as_ptr();
            let mut receiver = XdrReplyReceiver::<Metadata, _>::new(buffer);
            let mut wire = (data.len() as u32).to_be_bytes().to_vec();
            wire.extend_from_slice(data);
            wire.resize(wire.len().next_multiple_of(4), 0);
            feed(&mut receiver, &wire).unwrap();
            assert!(receiver.buffer().unwrap().is_empty());
            let outcome = receiver.finish(Ok(()));
            assert_eq!(outcome.result.unwrap(), Metadata(data.len() as u32));
            assert_eq!(outcome.buffer.as_ptr(), original);
            assert_eq!(outcome.payload_len, data.len());
            assert_eq!(&outcome.buffer[..data.len()], data);
        }
    }

    #[test]
    fn direct_receiver_rejects_large_data_bad_padding_and_truncation() {
        let mut receiver = XdrReplyReceiver::<Metadata, _>::new(vec![0; 2]);
        assert!(feed(&mut receiver, &3u32.to_be_bytes()).is_err());
        let mut receiver = XdrReplyReceiver::<Metadata, _>::new(vec![0; 2]);
        assert!(feed(&mut receiver, &[0, 0, 0, 1, b'x', 0, 1]).is_err());
        let mut receiver = XdrReplyReceiver::<Metadata, _>::new(vec![0; 2]);
        feed(&mut receiver, &[0, 0, 0, 2, b'x']).unwrap();
        let outcome = receiver.finish(Ok(()));
        assert!(outcome.result.is_err());
        assert_eq!(outcome.payload_len, 1);
        assert_eq!(outcome.buffer[0], b'x');
        let receiver = XdrReplyReceiver::<Metadata, _>::new(vec![0; 2]);
        assert!(receiver.finish(Ok(())).result.is_err());
    }

    #[tokio::test]
    async fn typed_admission_returns_owners_and_retry_preserves_auth() {
        let (local, mut peer) = duplex(512);
        let config = ClientConfig::new("127.0.0.1:2049".parse().unwrap());
        let transport = OwnedRpcTransport::from_stream_with_admission(
            local,
            &config,
            RecordLimits::default(),
            AdmissionLimits {
                max_calls: 1,
                max_bytes: 16384,
            },
        )
        .unwrap();
        let client = AsyncClient::new(config, transport.clone());
        let program = ProgramVersion {
            program: 42,
            version: 1,
        };
        let credentials = OpaqueAuth {
            flavor: onc_rpc_wire::AuthFlavor::Sys,
            body: Bytes::from_static(b"auth"),
        };
        let options = CallOptions::new().with_credentials(credentials.clone());
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let request = read_rpc_message(&mut peer).await.unwrap();
                let MessageBody::Call(call) = request.body else {
                    panic!("expected call")
                };
                assert_eq!(call.credentials, credentials);
                assert_eq!(call.payload.as_ref(), &[0, 0, 0, 9]);
                let reply = RpcMessage {
                    xid: request.xid,
                    body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                        verifier: OpaqueAuth::none(),
                        status: AcceptedStatus::Success(Bytes::from_static(&[
                            0, 0, 0, 1, b'x', 0, 0, 0,
                        ])),
                    })),
                };
                write_rpc_message(&mut peer, &reply).await.unwrap();
            }
        });
        let first = client
            .try_call_xdr_into::<_, Metadata, _>(program, Procedure(1), &9u32, vec![0; 8], &options)
            .unwrap();
        let buffer = vec![0; 8];
        let original = buffer.as_ptr();
        let refused = match client.try_call_xdr_into::<_, Metadata, _>(
            program,
            Procedure(1),
            &9u32,
            buffer,
            &options,
        ) {
            Err(refused) => refused,
            Ok(_) => panic!("second call must be refused"),
        };
        assert_eq!(refused.reason, AdmissionError::AtCapacity);
        assert!(!transport.is_closed());
        assert_eq!(first.await.result.unwrap(), Metadata(1));
        let outcome = client
            .try_call_xdr_into::<_, Metadata, _>(
                program,
                Procedure(1),
                refused.request,
                refused.receiver,
                &options,
            )
            .unwrap()
            .await;
        assert_eq!(outcome.result.unwrap(), Metadata(1));
        assert_eq!(outcome.buffer.as_ptr(), original);
        assert_eq!(outcome.buffer[0], b'x');
        server.await.unwrap();
    }

    #[tokio::test]
    async fn typed_segmented_call_preserves_owners_and_classifies_rpc_errors() {
        for reject in [false, true] {
            let (local, mut peer) = duplex(512);
            let config = ClientConfig::new("127.0.0.1:2049".parse().unwrap());
            let transport = OwnedRpcTransport::from_stream(local, &config).unwrap();
            let client = AsyncClient::new(config, transport);
            let server = tokio::spawn(async move {
                let request = read_rpc_message(&mut peer).await.unwrap();
                let MessageBody::Call(call) = request.body else {
                    panic!("expected call")
                };
                assert_eq!(call.payload.as_ref(), &[0, 0, 0, 3, b'a', b'b', b'c', 0]);
                let reply = RpcMessage {
                    xid: request.xid,
                    body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                        verifier: OpaqueAuth::none(),
                        status: if reject {
                            AcceptedStatus::ProcedureUnavailable
                        } else {
                            AcceptedStatus::Success(Bytes::from_static(&[0, 0, 0, 42]))
                        },
                    })),
                };
                write_rpc_message(&mut peer, &reply).await.unwrap();
            });
            let buffer = vec![0; 16];
            let original = buffer.as_ptr();
            let argument = Bytes::from_static(b"abc");
            let outcome = client
                .call_xdr_segmented::<_, u32, _>(
                    ProgramVersion {
                        program: 42,
                        version: 1,
                    },
                    Procedure(1),
                    &argument,
                    buffer,
                    &CallOptions::default(),
                )
                .await;
            assert_eq!(outcome.buffer.as_ptr(), original);
            if reject {
                assert!(matches!(
                    outcome.result,
                    Err(RuntimeError::ProcedureUnavailable)
                ));
            } else {
                assert_eq!(outcome.result.unwrap(), 42);
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn encode_failure_returns_the_original_destination_before_io() {
        struct Invalid(Bytes);
        impl XdrEncodeSegments for Invalid {
            fn encode_xdr_segments(
                &self,
                _: &mut onc_rpc_xdr::SegmentEncoder,
            ) -> Result<(), XdrError> {
                Err(XdrError::InvalidBool(2))
            }
        }
        let (local, _peer) = duplex(128);
        let config = ClientConfig::new("127.0.0.1:2049".parse().unwrap());
        let transport = OwnedRpcTransport::from_stream(local, &config).unwrap();
        let client = AsyncClient::new(config, transport.clone());
        let buffer = vec![0; 16];
        let original = buffer.as_ptr();
        let argument = Invalid(Bytes::from_owner(vec![42; 16]));
        let original_request = argument.0.as_ptr();
        let refused = match client.try_call_xdr_into::<_, Metadata, _>(
            ProgramVersion {
                program: 42,
                version: 1,
            },
            Procedure(1),
            argument,
            buffer,
            &CallOptions::default(),
        ) {
            Err(refused) => refused,
            Ok(_) => panic!("invalid argument must be refused"),
        };
        assert!(matches!(
            refused.reason,
            AdmissionError::InvalidRequest(RuntimeError::Encode(_))
        ));
        assert_eq!(refused.request.0.as_ptr(), original_request);
        assert_eq!(refused.receiver.as_ptr(), original);
        assert_eq!(transport.admission_snapshot().calls, 0);
        assert!(!transport.is_closed());
    }

    #[tokio::test]
    async fn xid_exhaustion_returns_typed_input_and_destination_without_io() {
        let (local, _peer) = duplex(128);
        let config = ClientConfig::new("127.0.0.1:2049".parse().unwrap());
        let transport = OwnedRpcTransport::from_stream(local, &config).unwrap();
        let client = AsyncClient::new(config, transport.clone());
        client
            .next_xid
            .0
            .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        let argument = Bytes::from_owner(vec![42; 16]);
        let original_request = argument.as_ptr();
        let buffer = vec![0; 16];
        let original = buffer.as_ptr();
        let refused = match client.try_call_xdr_segmented::<_, u32, _>(
            ProgramVersion {
                program: 42,
                version: 1,
            },
            Procedure(1),
            argument,
            buffer,
            &CallOptions::default(),
        ) {
            Err(refused) => refused,
            Ok(_) => panic!("exhausted client must refuse"),
        };
        assert!(matches!(
            refused.reason,
            AdmissionError::InvalidRequest(RuntimeError::Transport(_))
        ));
        assert_eq!(refused.request.as_ptr(), original_request);
        assert_eq!(refused.receiver.as_ptr(), original);
        assert_eq!(transport.admission_snapshot().calls, 0);
        assert!(!transport.is_closed());
    }
}
