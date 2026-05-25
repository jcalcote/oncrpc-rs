//! Minimal client-side ONC RPC contracts for generated stubs.

mod transport;

pub use async_trait::async_trait;
use bytes::{Buf, Bytes, BytesMut};
pub use onc_rpc_auth as auth;
pub use onc_rpc_wire::{
    AcceptedReply, AcceptedStatus, AuthStat, MAX_FRAGMENT_LEN, MessageBody, OpaqueAuth, Procedure,
    ProgramVersion, RecordMarker, RejectedReply, ReplyBody, RpcMessage, VersionRange, WireError,
    Xid, fragment_record,
};
use onc_rpc_xdr::{XdrDecode, XdrEncode};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub use transport::{TokioAsyncClientTransport, TokioClientTransport};

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub remote_addr: SocketAddr,
    pub local_addr: Option<SocketAddr>,
    pub connect_timeout: Duration,
    pub default_call_timeout: Option<Duration>,
    pub write_timeout: Option<Duration>,
    pub service_name: Option<String>,
    pub credentials: OpaqueAuth,
    pub verifier: OpaqueAuth,
}

impl ClientConfig {
    pub fn new(remote_addr: SocketAddr) -> Self {
        Self {
            remote_addr,
            local_addr: None,
            connect_timeout: Duration::from_secs(30),
            default_call_timeout: None,
            write_timeout: None,
            service_name: None,
            credentials: OpaqueAuth::none(),
            verifier: OpaqueAuth::none(),
        }
    }

    pub fn with_local_addr(mut self, local_addr: SocketAddr) -> Self {
        self.local_addr = Some(local_addr);
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_default_call_timeout(mut self, timeout: Duration) -> Self {
        self.default_call_timeout = Some(timeout);
        self
    }

    pub fn without_default_call_timeout(mut self) -> Self {
        self.default_call_timeout = None;
        self
    }

    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = Some(timeout);
        self
    }

    pub fn with_credentials(mut self, credentials: OpaqueAuth) -> Self {
        self.credentials = credentials;
        self
    }

    pub fn with_verifier(mut self, verifier: OpaqueAuth) -> Self {
        self.verifier = verifier;
        self
    }

    pub fn with_auth_none(mut self) -> Self {
        self.credentials = OpaqueAuth::none();
        self.verifier = OpaqueAuth::none();
        self
    }

    pub fn with_auth_sys(
        mut self,
        auth_sys: &onc_rpc_auth::AuthSys,
    ) -> Result<Self, onc_rpc_auth::AuthError> {
        self.credentials = auth_sys.to_opaque_auth()?;
        self.verifier = OpaqueAuth::none();
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRequest {
    pub program: ProgramVersion,
    pub procedure: Procedure,
    pub credentials: OpaqueAuth,
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

impl CallRequest {
    pub fn new(program: ProgramVersion, procedure: Procedure, payload: Bytes) -> Self {
        Self {
            program,
            procedure,
            credentials: OpaqueAuth::none(),
            verifier: OpaqueAuth::none(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallResponse {
    pub xid: Xid,
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CallTimeout {
    #[default]
    Inherit,
    None,
    Duration(Duration),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallOptions {
    pub timeout: CallTimeout,
    pub credentials: Option<OpaqueAuth>,
    pub verifier: Option<OpaqueAuth>,
}

impl CallOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = CallTimeout::Duration(timeout);
        self
    }

    pub fn without_timeout(mut self) -> Self {
        self.timeout = CallTimeout::None;
        self
    }

    pub fn effective_timeout(&self, default_timeout: Option<Duration>) -> Option<Duration> {
        match self.timeout {
            CallTimeout::Inherit => default_timeout,
            CallTimeout::None => None,
            CallTimeout::Duration(timeout) => Some(timeout),
        }
    }

    pub fn with_credentials(mut self, credentials: OpaqueAuth) -> Self {
        self.credentials = Some(credentials);
        self
    }

    pub fn with_verifier(mut self, verifier: OpaqueAuth) -> Self {
        self.verifier = Some(verifier);
        self
    }

    pub fn with_auth_none(mut self) -> Self {
        self.credentials = Some(OpaqueAuth::none());
        self.verifier = Some(OpaqueAuth::none());
        self
    }

    pub fn with_auth_sys(
        mut self,
        auth_sys: &onc_rpc_auth::AuthSys,
    ) -> Result<Self, onc_rpc_auth::AuthError> {
        self.credentials = Some(auth_sys.to_opaque_auth()?);
        self.verifier = Some(OpaqueAuth::none());
        Ok(self)
    }
}

pub trait ClientTransport: Send + Sync + 'static {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError>;

    fn call_with_options(
        &self,
        request: RpcMessage,
        _options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        self.call(request)
    }
}

#[async_trait]
pub trait AsyncClientTransport: Send + Sync + 'static {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError>;

    async fn call_with_options(
        &self,
        request: RpcMessage,
        _options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        self.call(request).await
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("transport failure: {0}")]
    Transport(String),
    #[error("connection closed before a complete reply was received")]
    ConnectionClosed,
    #[error("wire error: {0}")]
    Wire(WireError),
    #[error("response xid mismatch: expected {expected:?}, got {actual:?}")]
    XidMismatch { expected: Xid, actual: Xid },
    #[error("received an rpc call message where a reply was expected")]
    UnexpectedCallMessage,
    #[error("remote program is unavailable")]
    ProgramUnavailable,
    #[error("remote program version mismatch: supported range {0:?}")]
    ProgramMismatch(VersionRange),
    #[error("remote procedure is unavailable")]
    ProcedureUnavailable,
    #[error("remote reported garbage arguments")]
    GarbageArgs,
    #[error("remote reported a system error")]
    SystemError,
    #[error("rpc version mismatch: supported range {0:?}")]
    RpcMismatch(VersionRange),
    #[error("rpc auth error: {0:?}")]
    AuthError(AuthStat),
    #[error("failed to encode XDR payload: {0}")]
    Encode(onc_rpc_xdr::XdrError),
    #[error("failed to decode XDR payload: {0}")]
    Decode(onc_rpc_xdr::XdrError),
}

pub struct Client<T> {
    config: ClientConfig,
    transport: T,
    next_xid: AtomicU32,
}

pub struct AsyncClient<T> {
    config: ClientConfig,
    transport: T,
    next_xid: AtomicU32,
}

impl<T> Client<T>
where
    T: ClientTransport,
{
    pub fn new(config: ClientConfig, transport: T) -> Self {
        Self {
            config,
            transport,
            next_xid: AtomicU32::new(1),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub fn call(&self, request: CallRequest) -> Result<CallResponse, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
    }

    pub fn call_with_options(
        &self,
        request: CallRequest,
        options: &CallOptions,
    ) -> Result<CallResponse, RuntimeError> {
        let xid = Xid(self.next_xid.fetch_add(1, Ordering::Relaxed));
        let wire_request = build_request_message_with_options(&self.config, xid, request, options);
        let reply = self.transport.call_with_options(wire_request, options)?;
        handle_reply(xid, reply)
    }

    pub fn call_typed<Arg, Ret>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: &Arg,
    ) -> Result<Ret, RuntimeError>
    where
        Arg: XdrEncode,
        Ret: XdrDecode,
    {
        self.call_typed_with_options(program, procedure, argument, &CallOptions::default())
    }

    pub fn call_typed_with_options<Arg, Ret>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: &Arg,
        options: &CallOptions,
    ) -> Result<Ret, RuntimeError>
    where
        Arg: XdrEncode,
        Ret: XdrDecode,
    {
        let payload = argument.to_xdr_bytes().map_err(RuntimeError::Encode)?;
        let response =
            self.call_with_options(CallRequest::new(program, procedure, payload), options)?;
        Ret::from_xdr_bytes(&response.payload).map_err(RuntimeError::Decode)
    }
}

impl<T> AsyncClient<T>
where
    T: AsyncClientTransport,
{
    pub fn new(config: ClientConfig, transport: T) -> Self {
        Self {
            config,
            transport,
            next_xid: AtomicU32::new(1),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub async fn call(&self, request: CallRequest) -> Result<CallResponse, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
            .await
    }

    pub async fn call_with_options(
        &self,
        request: CallRequest,
        options: &CallOptions,
    ) -> Result<CallResponse, RuntimeError> {
        let xid = Xid(self.next_xid.fetch_add(1, Ordering::Relaxed));
        let wire_request = build_request_message_with_options(&self.config, xid, request, options);
        let reply = self
            .transport
            .call_with_options(wire_request, options)
            .await?;
        handle_reply(xid, reply)
    }

    pub async fn call_typed<Arg, Ret>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: &Arg,
    ) -> Result<Ret, RuntimeError>
    where
        Arg: XdrEncode,
        Ret: XdrDecode,
    {
        self.call_typed_with_options(program, procedure, argument, &CallOptions::default())
            .await
    }

    pub async fn call_typed_with_options<Arg, Ret>(
        &self,
        program: ProgramVersion,
        procedure: Procedure,
        argument: &Arg,
        options: &CallOptions,
    ) -> Result<Ret, RuntimeError>
    where
        Arg: XdrEncode,
        Ret: XdrDecode,
    {
        let payload = argument.to_xdr_bytes().map_err(RuntimeError::Encode)?;
        let response = self
            .call_with_options(CallRequest::new(program, procedure, payload), options)
            .await?;
        Ret::from_xdr_bytes(&response.payload).map_err(RuntimeError::Decode)
    }
}

fn build_request_message(config: &ClientConfig, xid: Xid, request: CallRequest) -> RpcMessage {
    let credentials = if request.credentials == OpaqueAuth::none() {
        config.credentials.clone()
    } else {
        request.credentials
    };

    let verifier = if request.verifier == OpaqueAuth::none() {
        config.verifier.clone()
    } else {
        request.verifier
    };

    RpcMessage {
        xid,
        body: MessageBody::Call(onc_rpc_wire::CallBody::new(
            request.program,
            request.procedure,
            credentials,
            verifier,
            request.payload,
        )),
    }
}

fn build_request_message_with_options(
    config: &ClientConfig,
    xid: Xid,
    request: CallRequest,
    options: &CallOptions,
) -> RpcMessage {
    let mut message = build_request_message(config, xid, request);
    let MessageBody::Call(call) = &mut message.body else {
        unreachable!("request message must be a call");
    };
    if let Some(credentials) = &options.credentials {
        call.credentials = credentials.clone();
    }
    if let Some(verifier) = &options.verifier {
        call.verifier = verifier.clone();
    }
    message
}

fn handle_reply(xid: Xid, reply: RpcMessage) -> Result<CallResponse, RuntimeError> {
    if reply.xid != xid {
        return Err(RuntimeError::XidMismatch {
            expected: xid,
            actual: reply.xid,
        });
    }

    match reply.body {
        MessageBody::Call(_) => Err(RuntimeError::UnexpectedCallMessage),
        MessageBody::Reply(ReplyBody::Accepted(AcceptedReply { verifier, status })) => match status
        {
            AcceptedStatus::Success(payload) => Ok(CallResponse {
                xid,
                verifier,
                payload,
            }),
            AcceptedStatus::ProgramUnavailable => Err(RuntimeError::ProgramUnavailable),
            AcceptedStatus::ProgramMismatch(range) => Err(RuntimeError::ProgramMismatch(range)),
            AcceptedStatus::ProcedureUnavailable => Err(RuntimeError::ProcedureUnavailable),
            AcceptedStatus::GarbageArgs => Err(RuntimeError::GarbageArgs),
            AcceptedStatus::SystemError => Err(RuntimeError::SystemError),
        },
        MessageBody::Reply(ReplyBody::Denied(RejectedReply::RpcMismatch(range))) => {
            Err(RuntimeError::RpcMismatch(range))
        }
        MessageBody::Reply(ReplyBody::Denied(RejectedReply::AuthError(status))) => {
            Err(RuntimeError::AuthError(status))
        }
    }
}

pub fn try_decode_message_from_buffer(
    buffer: &mut BytesMut,
) -> Result<Option<RpcMessage>, RuntimeError> {
    let Some(record) = try_take_record_from_buffer(buffer)? else {
        return Ok(None);
    };
    RpcMessage::decode(&record)
        .map(Some)
        .map_err(RuntimeError::Wire)
}

pub async fn write_rpc_message<W>(writer: &mut W, message: &RpcMessage) -> Result<(), RuntimeError>
where
    W: AsyncWrite + Unpin,
{
    let payload = message.encode().map_err(RuntimeError::Wire)?;
    write_record_payload(writer, &payload).await
}

fn try_take_record_from_buffer(buffer: &mut BytesMut) -> Result<Option<Bytes>, RuntimeError> {
    let mut offset = 0usize;
    let mut fragments = 0usize;
    let mut total_payload_len = 0usize;

    loop {
        let remaining = &buffer[offset..];
        if remaining.is_empty() {
            return Ok(None);
        }
        if remaining.len() < 4 {
            return Ok(None);
        }

        let marker =
            RecordMarker::decode_bytes(remaining[..4].try_into().expect("slice len checked"));
        let fragment_len = marker.payload_len as usize;
        let consumed = 4 + fragment_len;

        if remaining.len() < consumed {
            return Ok(None);
        }

        fragments += 1;
        total_payload_len += fragment_len;
        offset += consumed;

        if marker.last_fragment {
            break;
        }
    }

    if fragments == 1 {
        let mut framed = buffer.split_to(offset);
        framed.advance(4);
        return Ok(Some(framed.freeze()));
    }

    let mut consumed = buffer.split_to(offset);
    let mut assembled = BytesMut::with_capacity(total_payload_len);
    while !consumed.is_empty() {
        let marker =
            RecordMarker::decode_bytes(consumed[..4].try_into().expect("slice len checked"));
        consumed.advance(4);
        let fragment_len = marker.payload_len as usize;
        assembled.extend_from_slice(&consumed[..fragment_len]);
        consumed.advance(fragment_len);
        if marker.last_fragment {
            break;
        }
    }

    Ok(Some(assembled.freeze()))
}

async fn write_record_payload<W>(writer: &mut W, payload: &Bytes) -> Result<(), RuntimeError>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() as u64 <= MAX_FRAGMENT_LEN as u64 {
        let marker = RecordMarker::new(payload.len() as u32, true).map_err(RuntimeError::Wire)?;
        writer
            .write_all(&marker.encode_bytes())
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        writer
            .write_all(payload)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        return Ok(());
    }

    for fragment in fragment_record(payload, MAX_FRAGMENT_LEN).map_err(RuntimeError::Wire)? {
        writer
            .write_all(&fragment)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
    }
    writer
        .flush()
        .await
        .map_err(|err| RuntimeError::Transport(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeTransport {
        replies: Mutex<VecDeque<Result<RpcMessage, RuntimeError>>>,
    }

    impl FakeTransport {
        fn new(replies: Vec<Result<RpcMessage, RuntimeError>>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
            }
        }
    }

    impl ClientTransport for FakeTransport {
        fn call(&self, _request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
            self.replies
                .lock()
                .expect("mutex poisoned")
                .pop_front()
                .expect("test reply should exist")
        }
    }

    #[async_trait]
    impl AsyncClientTransport for FakeTransport {
        async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
            ClientTransport::call(self, request)
        }
    }

    fn config() -> ClientConfig {
        ClientConfig::new("127.0.0.1:2049".parse().expect("valid socket"))
    }

    fn request() -> CallRequest {
        CallRequest::new(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            Bytes::from_static(b"payload"),
        )
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct EchoValue(u32);

    #[test]
    fn call_options_resolve_inherit_none_and_override() {
        assert_eq!(
            CallOptions::default().effective_timeout(Some(Duration::from_secs(5))),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            CallOptions::new()
                .without_timeout()
                .effective_timeout(Some(Duration::from_secs(5))),
            None
        );
        assert_eq!(
            CallOptions::new()
                .with_timeout(Duration::from_secs(9))
                .effective_timeout(Some(Duration::from_secs(5))),
            Some(Duration::from_secs(9))
        );
        assert_eq!(
            CallOptions::default(),
            CallOptions {
                timeout: CallTimeout::Inherit,
                credentials: None,
                verifier: None,
            }
        );
    }

    #[test]
    fn client_config_and_call_options_can_encode_auth_sys() {
        let auth_sys = onc_rpc_auth::AuthSys {
            stamp: 9,
            machine_name: "runtime-client".into(),
            uid: 1000,
            gid: 100,
            gids: vec![101, 102],
        };

        let config = config()
            .with_auth_sys(&auth_sys)
            .expect("auth sys should encode");
        assert_eq!(
            onc_rpc_auth::decode_auth(&config.credentials).expect("decode should succeed"),
            onc_rpc_auth::AuthFlavor::Sys(auth_sys.clone())
        );
        assert_eq!(config.verifier, OpaqueAuth::none());

        let options = CallOptions::new()
            .with_auth_sys(&auth_sys)
            .expect("auth sys should encode");
        assert_eq!(
            onc_rpc_auth::decode_auth(
                options
                    .credentials
                    .as_ref()
                    .expect("credentials should be present")
            )
            .expect("decode should succeed"),
            onc_rpc_auth::AuthFlavor::Sys(auth_sys)
        );
        assert_eq!(options.verifier, Some(OpaqueAuth::none()));
    }

    impl XdrEncode for EchoValue {
        fn encode_xdr(&self, output: &mut bytes::BytesMut) -> Result<(), onc_rpc_xdr::XdrError> {
            self.0.encode_xdr(output)
        }
    }

    impl XdrDecode for EchoValue {
        fn decode_xdr(input: &mut &[u8]) -> Result<Self, onc_rpc_xdr::XdrError> {
            Ok(Self(u32::decode_xdr(input)?))
        }
    }

    #[test]
    fn client_call_returns_success_payload() {
        let reply = RpcMessage {
            xid: Xid(1),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(Bytes::from_static(b"reply")),
            })),
        };
        let client = Client::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let response = client.call(request()).expect("call should succeed");

        assert_eq!(response.xid, Xid(1));
        assert_eq!(response.payload, Bytes::from_static(b"reply"));
    }

    #[test]
    fn client_call_rejects_xid_mismatch() {
        let reply = RpcMessage {
            xid: Xid(999),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(Bytes::new()),
            })),
        };
        let client = Client::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let error = client.call(request()).expect_err("xid mismatch must fail");

        assert_eq!(
            error,
            RuntimeError::XidMismatch {
                expected: Xid(1),
                actual: Xid(999),
            }
        );
    }

    #[test]
    fn client_call_maps_program_unavailable_reply() {
        let reply = RpcMessage {
            xid: Xid(1),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::ProgramUnavailable,
            })),
        };
        let client = Client::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let error = client
            .call(request())
            .expect_err("program unavailable must fail");

        assert_eq!(error, RuntimeError::ProgramUnavailable);
    }

    #[test]
    fn client_call_typed_encodes_request_and_decodes_reply() {
        let reply_payload = EchoValue(99)
            .to_xdr_bytes()
            .expect("reply payload should encode");
        let reply = RpcMessage {
            xid: Xid(1),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(reply_payload),
            })),
        };
        let client = Client::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let response: EchoValue = client
            .call_typed(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(1),
                &EchoValue(7),
            )
            .expect("typed call should succeed");

        assert_eq!(response, EchoValue(99));
    }

    #[test]
    fn client_call_typed_reports_decode_failures() {
        let reply = RpcMessage {
            xid: Xid(1),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(Bytes::from_static(&[0, 0, 0])),
            })),
        };
        let client = Client::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let error = client
            .call_typed::<(), EchoValue>(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(1),
                &(),
            )
            .expect_err("invalid XDR should fail decode");

        assert!(matches!(
            error,
            RuntimeError::Decode(onc_rpc_xdr::XdrError::UnexpectedEof { .. })
        ));
    }

    #[tokio::test]
    async fn async_client_call_returns_success_payload() {
        let reply = RpcMessage {
            xid: Xid(1),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(Bytes::from_static(b"reply")),
            })),
        };
        let client = AsyncClient::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let response = client.call(request()).await.expect("call should succeed");

        assert_eq!(response.xid, Xid(1));
        assert_eq!(response.payload, Bytes::from_static(b"reply"));
    }

    #[tokio::test]
    async fn async_client_call_typed_encodes_request_and_decodes_reply() {
        let reply_payload = EchoValue(321)
            .to_xdr_bytes()
            .expect("reply payload should encode");
        let reply = RpcMessage {
            xid: Xid(1),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(reply_payload),
            })),
        };
        let client = AsyncClient::new(config(), FakeTransport::new(vec![Ok(reply)]));

        let response: EchoValue = client
            .call_typed(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(1),
                &EchoValue(123),
            )
            .await
            .expect("typed call should succeed");

        assert_eq!(response, EchoValue(321));
    }
}
