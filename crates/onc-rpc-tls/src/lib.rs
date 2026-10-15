//! TLS and STARTTLS integration points for ONC RPC.

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClientTransport, AuthStat, CallOptions, ClientConfig,
    ClientTransport, MessageBody, OpaqueAuth, OwnedRpcTransport, Procedure, ProgramVersion,
    RecordLimits, RejectedReply, ReplyBody, RpcMessage, RuntimeError, Xid,
    read_rpc_message_with_limits, try_decode_message_from_buffer, write_rpc_message,
};
use onc_rpc_server::{AsyncServer, Server, ServerError, ServerTransportError};
use onc_rpc_wire::AuthFlavor;
use rustls::pki_types::ServerName;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, oneshot};
use tokio::time::timeout;
use tokio_rustls::rustls;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const AUTH_TLS_VALUE: u32 = 7;
const SUNRPC_ALPN: &[u8] = b"sunrpc";
const STARTTLS_TOKEN: &[u8; 8] = b"STARTTLS";
// A STARTTLS NULL reply has no application payload. Leave room for the full
// RPC auth envelope and fragmentation without the generic 128 MiB buffer.
const STARTTLS_PROBE_LIMITS: RecordLimits = RecordLimits {
    max_record_bytes: 4096,
    max_fragments: 16,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    Disabled,
    Allowed,
    Required,
    MutualAuthRequired,
}

#[derive(Debug, Clone)]
pub struct ClientTlsConfig {
    pub tls_config: Arc<rustls::ClientConfig>,
    pub server_name: ServerName<'static>,
}

impl ClientTlsConfig {
    pub fn new(
        mut tls_config: rustls::ClientConfig,
        server_name: impl Into<ServerName<'static>>,
    ) -> Self {
        tls_config.alpn_protocols = vec![SUNRPC_ALPN.to_vec()];
        Self {
            tls_config: Arc::new(tls_config),
            server_name: server_name.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerTlsConfig {
    pub tls_config: Arc<rustls::ServerConfig>,
}

impl ServerTlsConfig {
    pub fn new(mut tls_config: rustls::ServerConfig) -> Self {
        tls_config.alpn_protocols = vec![SUNRPC_ALPN.to_vec()];
        Self {
            tls_config: Arc::new(tls_config),
        }
    }
}

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("runtime failure: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("server transport failure: {0}")]
    ServerTransport(#[from] ServerTransportError),
    #[error("server dispatch failure: {0}")]
    Server(#[from] ServerError),
    #[error("tls handshake failure: {0}")]
    Handshake(String),
    #[error("tls server name is invalid for this endpoint")]
    InvalidServerName,
    #[error("rpc-with-tls peer did not negotiate ALPN sunrpc")]
    MissingSunrpcAlpn,
    #[error("starttls negotiation was rejected")]
    StartTlsRejected,
    #[error("starttls probe received malformed reply")]
    InvalidStartTlsReply,
}

#[derive(Clone)]
pub struct TokioAsyncTlsClientTransport {
    inner: OwnedRpcTransport,
}

pub struct TokioTlsClientTransport {
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: TokioAsyncTlsClientTransport,
}

#[derive(Clone)]
pub struct TokioAsyncStartTlsClientTransport {
    inner: OwnedRpcTransport,
}

pub struct TokioStartTlsClientTransport {
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: TokioAsyncStartTlsClientTransport,
}

pub struct TokioTlsServerTransport {
    listener: TcpListener,
    server: Arc<Server>,
    tls_acceptor: TlsAcceptor,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

pub struct TokioAsyncTlsServerTransport {
    listener: TcpListener,
    server: Arc<AsyncServer>,
    tls_acceptor: TlsAcceptor,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

pub struct TokioStartTlsServerTransport {
    listener: TcpListener,
    server: Arc<Server>,
    tls_acceptor: TlsAcceptor,
    mode: TlsMode,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

pub struct TokioAsyncStartTlsServerTransport {
    listener: TcpListener,
    server: Arc<AsyncServer>,
    tls_acceptor: TlsAcceptor,
    mode: TlsMode,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

impl TokioAsyncTlsClientTransport {
    pub fn close(&self) {
        self.inner.close();
    }
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
    pub async fn wait_closed(&self) {
        self.inner.wait_closed().await;
    }

    pub async fn connect(config: &ClientConfig, tls: &ClientTlsConfig) -> Result<Self, TlsError> {
        setup_with_timeout(config, async {
            let stream = tcp_connect(config).await?;
            let connector = TlsConnector::from(tls.tls_config.clone());
            let tls_stream = connector
                .connect(tls.server_name.clone(), stream)
                .await
                .map_err(|err| TlsError::Handshake(err.to_string()))?;
            validate_client_alpn(&tls_stream)?;
            Ok(Self {
                inner: OwnedRpcTransport::from_stream(tls_stream, config)?,
            })
        })
        .await
    }
}

impl TokioTlsClientTransport {
    pub fn connect(config: &ClientConfig, tls: &ClientTlsConfig) -> Result<Self, TlsError> {
        let runtime = build_owned_runtime()?;
        let runtime_handle = runtime.handle().clone();
        let runtime_guard = Arc::new(OwnedRuntime::new(runtime));
        let inner = runtime_handle.block_on(TokioAsyncTlsClientTransport::connect(config, tls))?;
        Ok(Self {
            runtime_handle,
            runtime_guard,
            inner,
        })
    }
}

impl TokioAsyncStartTlsClientTransport {
    pub fn close(&self) {
        self.inner.close();
    }
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
    pub async fn wait_closed(&self) {
        self.inner.wait_closed().await;
    }

    pub async fn connect(
        config: &ClientConfig,
        tls: &ClientTlsConfig,
        program: ProgramVersion,
    ) -> Result<Self, TlsError> {
        setup_with_timeout(config, async {
            let mut stream = tcp_connect(config).await?;
            perform_starttls_probe(&mut stream, program).await?;
            let connector = TlsConnector::from(tls.tls_config.clone());
            let tls_stream = connector
                .connect(tls.server_name.clone(), stream)
                .await
                .map_err(|err| TlsError::Handshake(err.to_string()))?;
            validate_client_alpn(&tls_stream)?;
            Ok(Self {
                inner: OwnedRpcTransport::from_stream(tls_stream, config)?,
            })
        })
        .await
    }
}

impl TokioStartTlsClientTransport {
    pub fn connect(
        config: &ClientConfig,
        tls: &ClientTlsConfig,
        program: ProgramVersion,
    ) -> Result<Self, TlsError> {
        let runtime = build_owned_runtime()?;
        let runtime_handle = runtime.handle().clone();
        let runtime_guard = Arc::new(OwnedRuntime::new(runtime));
        let inner = runtime_handle.block_on(TokioAsyncStartTlsClientTransport::connect(
            config, tls, program,
        ))?;
        Ok(Self {
            runtime_handle,
            runtime_guard,
            inner,
        })
    }
}

impl TokioTlsServerTransport {
    pub async fn bind(server: Server, tls: ServerTlsConfig) -> Result<Self, TlsError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
            tls_acceptor: TlsAcceptor::from(tls.tls_config),
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TlsError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()).into())
    }

    pub async fn accept_once(&self) -> Result<(), TlsError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        let tls_acceptor = self.tls_acceptor.clone();
        run_sync_connection_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            tls_acceptor
                .accept(stream)
                .await
                .map_err(|err| TlsError::Handshake(err.to_string()))?,
            self.server.clone(),
            self.worker_limit.clone(),
        )
        .await?;
        Ok(())
    }

    pub async fn serve(self) -> Result<(), TlsError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            let worker_limit = self.worker_limit.clone();
            let runtime_guard = self.runtime_guard.clone();
            let tls_acceptor = self.tls_acceptor.clone();
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                if let Ok(stream) = tls_acceptor.accept(stream).await {
                    let _ = serve_sync_connection(stream, server, worker_limit).await;
                }
            });
        }
    }
}

impl TokioAsyncTlsServerTransport {
    pub async fn bind(server: AsyncServer, tls: ServerTlsConfig) -> Result<Self, TlsError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
            tls_acceptor: TlsAcceptor::from(tls.tls_config),
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TlsError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()).into())
    }

    pub async fn accept_once(&self) -> Result<(), TlsError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        let tls_acceptor = self.tls_acceptor.clone();
        run_async_connection_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            tls_acceptor
                .accept(stream)
                .await
                .map_err(|err| TlsError::Handshake(err.to_string()))?,
            self.server.clone(),
            self.worker_limit.clone(),
        )
        .await?;
        Ok(())
    }

    pub async fn serve(self) -> Result<(), TlsError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            let worker_limit = self.worker_limit.clone();
            let runtime_guard = self.runtime_guard.clone();
            let tls_acceptor = self.tls_acceptor.clone();
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                if let Ok(stream) = tls_acceptor.accept(stream).await {
                    let _ = serve_async_connection(stream, server, worker_limit).await;
                }
            });
        }
    }
}

impl TokioStartTlsServerTransport {
    pub async fn bind(
        server: Server,
        tls: ServerTlsConfig,
        mode: TlsMode,
    ) -> Result<Self, TlsError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
            tls_acceptor: TlsAcceptor::from(tls.tls_config),
            mode,
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TlsError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()).into())
    }

    pub async fn accept_once(&self) -> Result<(), TlsError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        let mode = self.mode;
        let acceptor = self.tls_acceptor.clone();
        let server = self.server.clone();
        let worker_limit = self.worker_limit.clone();
        let runtime_handle = self.runtime_handle.clone();
        let runtime_guard = self.runtime_guard.clone();
        let (tx, rx) = oneshot::channel();
        self.runtime_handle.spawn(async move {
            let _runtime_guard = runtime_guard;
            let _ = tx.send(
                serve_starttls_sync_connection(stream, server, worker_limit, acceptor, mode).await,
            );
        });
        let _ = runtime_handle;
        rx.await.map_err(|_| {
            TlsError::ServerTransport(ServerTransportError::Io("server runtime terminated".into()))
        })??;
        Ok(())
    }

    pub async fn serve(self) -> Result<(), TlsError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            let worker_limit = self.worker_limit.clone();
            let runtime_guard = self.runtime_guard.clone();
            let acceptor = self.tls_acceptor.clone();
            let mode = self.mode;
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                let _ =
                    serve_starttls_sync_connection(stream, server, worker_limit, acceptor, mode)
                        .await;
            });
        }
    }
}

impl TokioAsyncStartTlsServerTransport {
    pub async fn bind(
        server: AsyncServer,
        tls: ServerTlsConfig,
        mode: TlsMode,
    ) -> Result<Self, TlsError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
            tls_acceptor: TlsAcceptor::from(tls.tls_config),
            mode,
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TlsError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()).into())
    }

    pub async fn accept_once(&self) -> Result<(), TlsError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        let acceptor = self.tls_acceptor.clone();
        let mode = self.mode;
        run_async_starttls_connection_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            stream,
            self.server.clone(),
            self.worker_limit.clone(),
            acceptor,
            mode,
        )
        .await?;
        Ok(())
    }

    pub async fn serve(self) -> Result<(), TlsError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            let worker_limit = self.worker_limit.clone();
            let runtime_guard = self.runtime_guard.clone();
            let acceptor = self.tls_acceptor.clone();
            let mode = self.mode;
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                let _ =
                    serve_starttls_async_connection(stream, server, worker_limit, acceptor, mode)
                        .await;
            });
        }
    }
}

#[async_trait]
impl AsyncClientTransport for TokioAsyncTlsClientTransport {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.inner.call(request).await
    }
    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        self.inner.call_with_options(request, options).await
    }
}

impl ClientTransport for TokioTlsClientTransport {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
    }

    fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        sync_call(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            self.inner.clone(),
            request,
            options,
        )
    }
}

#[async_trait]
impl AsyncClientTransport for TokioAsyncStartTlsClientTransport {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.inner.call(request).await
    }
    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        self.inner.call_with_options(request, options).await
    }
}

impl ClientTransport for TokioStartTlsClientTransport {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
    }

    fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        sync_call(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            self.inner.clone(),
            request,
            options,
        )
    }
}

async fn setup_with_timeout<T>(
    config: &ClientConfig,
    setup: impl std::future::Future<Output = Result<T, TlsError>>,
) -> Result<T, TlsError> {
    timeout(config.connect_timeout, setup).await.map_err(|_| {
        TlsError::Runtime(RuntimeError::Transport(format!(
            "TLS setup timeout after {:?}",
            config.connect_timeout
        )))
    })?
}

async fn tcp_connect(config: &ClientConfig) -> Result<TcpStream, RuntimeError> {
    let socket = socket_for_remote(config.remote_addr)?;
    if let Some(local_addr) = config.local_addr {
        socket
            .bind(local_addr)
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
    }

    timeout(config.connect_timeout, socket.connect(config.remote_addr))
        .await
        .map_err(|_| {
            RuntimeError::Transport(format!(
                "connect timeout after {:?}",
                config.connect_timeout
            ))
        })?
        .map_err(|err| RuntimeError::Transport(err.to_string()))
}

fn socket_for_remote(remote_addr: SocketAddr) -> Result<TcpSocket, RuntimeError> {
    match remote_addr {
        SocketAddr::V4(_) => {
            TcpSocket::new_v4().map_err(|err| RuntimeError::Transport(err.to_string()))
        }
        SocketAddr::V6(_) => {
            TcpSocket::new_v6().map_err(|err| RuntimeError::Transport(err.to_string()))
        }
    }
}

fn validate_client_alpn(
    stream: &tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<(), TlsError> {
    match stream.get_ref().1.alpn_protocol() {
        Some(protocol) if protocol == SUNRPC_ALPN => Ok(()),
        _ => Err(TlsError::MissingSunrpcAlpn),
    }
}

async fn perform_starttls_probe(
    stream: &mut TcpStream,
    program: ProgramVersion,
) -> Result<(), TlsError> {
    let request = RpcMessage {
        xid: Xid(1),
        body: MessageBody::Call(onc_rpc_wire::CallBody::new(
            program,
            Procedure(0),
            OpaqueAuth::new(AuthFlavor::Unknown(AUTH_TLS_VALUE), Bytes::new())
                .map_err(RuntimeError::Wire)?,
            OpaqueAuth::none(),
            Bytes::new(),
        )),
    };
    write_rpc_message(stream, &request).await?;

    let reply = read_rpc_message_with_limits(stream, STARTTLS_PROBE_LIMITS)
        .await
        .map_err(|error| match error {
            RuntimeError::ConnectionClosed => TlsError::StartTlsRejected,
            error => TlsError::Runtime(error),
        })?;
    validate_starttls_reply(reply, request.xid)
}

fn validate_starttls_reply(reply: RpcMessage, expected_xid: Xid) -> Result<(), TlsError> {
    if reply.xid != expected_xid {
        return Err(TlsError::InvalidStartTlsReply);
    }
    match reply.body {
        MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier,
            status: AcceptedStatus::Success(payload),
        })) if verifier.flavor == AuthFlavor::None
            && verifier.body.as_ref() == STARTTLS_TOKEN
            && payload.is_empty() =>
        {
            Ok(())
        }
        MessageBody::Reply(ReplyBody::Denied(RejectedReply::AuthError(_))) => {
            Err(TlsError::StartTlsRejected)
        }
        _ => Err(TlsError::InvalidStartTlsReply),
    }
}

fn sync_call<T>(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: T,
    request: RpcMessage,
    options: &CallOptions,
) -> Result<RpcMessage, RuntimeError>
where
    T: AsyncClientTransport + Clone + Send + Sync + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    let options = options.clone();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ = tx.send(inner.call_with_options(request, &options).await);
    });
    rx.recv().map_err(|_| RuntimeError::ConnectionClosed)?
}

async fn serve_sync_connection<S>(
    stream: S,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    serve_sync_split(reader, writer, server, worker_limit, None, BytesMut::new()).await
}

async fn serve_async_connection<S>(
    stream: S,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    serve_async_split(reader, writer, server, worker_limit, None, BytesMut::new()).await
}

async fn serve_sync_split<R, W>(
    mut reader: R,
    writer: W,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
    initial_message: Option<RpcMessage>,
    mut buffer: BytesMut,
) -> Result<(), TlsError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let writer = Arc::new(Mutex::new(writer));
    if buffer.capacity() == 0 {
        buffer = BytesMut::with_capacity(8192);
    }
    if let Some(message) = initial_message {
        spawn_sync_reply(
            server.clone(),
            writer.clone(),
            worker_limit.clone(),
            message,
        )
        .await?;
    }

    loop {
        while let Some(message) = try_decode_message_from_buffer(&mut buffer)? {
            if auth_tls_request(&message).is_some() {
                let reply = auth_tls_badcred_reply(message.xid);
                let mut writer = writer.lock().await;
                write_rpc_message(&mut *writer, &reply).await?;
                continue;
            }

            spawn_sync_reply(
                server.clone(),
                writer.clone(),
                worker_limit.clone(),
                message,
            )
            .await?;
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()));
        let read = match read {
            Ok(read) => read,
            Err(RuntimeError::Transport(message)) if is_tls_close_without_notify(&message) => {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            return Ok(());
        }
    }
}

async fn serve_async_split<R, W>(
    mut reader: R,
    writer: W,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
    initial_message: Option<RpcMessage>,
    mut buffer: BytesMut,
) -> Result<(), TlsError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let writer = Arc::new(Mutex::new(writer));
    if buffer.capacity() == 0 {
        buffer = BytesMut::with_capacity(8192);
    }
    if let Some(message) = initial_message {
        spawn_async_reply(
            server.clone(),
            writer.clone(),
            worker_limit.clone(),
            message,
        )
        .await?;
    }

    loop {
        while let Some(message) = try_decode_message_from_buffer(&mut buffer)? {
            if auth_tls_request(&message).is_some() {
                let reply = auth_tls_badcred_reply(message.xid);
                let mut writer = writer.lock().await;
                write_rpc_message(&mut *writer, &reply).await?;
                continue;
            }

            spawn_async_reply(
                server.clone(),
                writer.clone(),
                worker_limit.clone(),
                message,
            )
            .await?;
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()));
        let read = match read {
            Ok(read) => read,
            Err(RuntimeError::Transport(message)) if is_tls_close_without_notify(&message) => {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            return Ok(());
        }
    }
}

async fn spawn_sync_reply<W>(
    server: Arc<Server>,
    writer: Arc<Mutex<W>>,
    worker_limit: Arc<Semaphore>,
    message: RpcMessage,
) -> Result<(), TlsError>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let permit = acquire_worker_permit(worker_limit).await?;
    tokio::spawn(async move {
        let _permit = permit;
        if let Ok(reply) = server.handle_message(message) {
            let mut writer = writer.lock().await;
            let _ = write_rpc_message(&mut *writer, &reply).await;
        }
    });
    Ok(())
}

async fn spawn_async_reply<W>(
    server: Arc<AsyncServer>,
    writer: Arc<Mutex<W>>,
    worker_limit: Arc<Semaphore>,
    message: RpcMessage,
) -> Result<(), TlsError>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let permit = acquire_worker_permit(worker_limit).await?;
    tokio::spawn(async move {
        let _permit = permit;
        if let Ok(reply) = server.handle_message(message).await {
            let mut writer = writer.lock().await;
            let _ = write_rpc_message(&mut *writer, &reply).await;
        }
    });
    Ok(())
}

async fn serve_starttls_sync_connection(
    mut stream: TcpStream,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
    acceptor: TlsAcceptor,
    mode: TlsMode,
) -> Result<(), TlsError> {
    let mut buffer = BytesMut::with_capacity(8192);
    loop {
        if let Some(message) = try_decode_message_from_buffer(&mut buffer)? {
            match auth_tls_request(&message) {
                Some(AuthTlsRequest::Probe) => {
                    write_rpc_message(&mut stream, &starttls_ok_reply(message.xid)?).await?;
                    let tls_stream = acceptor
                        .accept(stream)
                        .await
                        .map_err(|err| TlsError::Handshake(err.to_string()))?;
                    return serve_sync_connection(tls_stream, server, worker_limit).await;
                }
                Some(AuthTlsRequest::Invalid) => {
                    write_rpc_message(&mut stream, &auth_tls_badcred_reply(message.xid)).await?;
                    continue;
                }
                None => {
                    if matches!(mode, TlsMode::Required | TlsMode::MutualAuthRequired) {
                        write_rpc_message(&mut stream, &auth_tls_badcred_reply(message.xid))
                            .await?;
                        return Ok(());
                    }
                    let (reader, writer) = tokio::io::split(stream);
                    return serve_sync_split(
                        reader,
                        writer,
                        server,
                        worker_limit,
                        Some(message),
                        buffer,
                    )
                    .await;
                }
            }
        }

        let read = stream
            .read_buf(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        if read == 0 {
            return Ok(());
        }
    }
}

async fn serve_starttls_async_connection(
    mut stream: TcpStream,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
    acceptor: TlsAcceptor,
    mode: TlsMode,
) -> Result<(), TlsError> {
    let mut buffer = BytesMut::with_capacity(8192);
    loop {
        if let Some(message) = try_decode_message_from_buffer(&mut buffer)? {
            match auth_tls_request(&message) {
                Some(AuthTlsRequest::Probe) => {
                    write_rpc_message(&mut stream, &starttls_ok_reply(message.xid)?).await?;
                    let tls_stream = acceptor
                        .accept(stream)
                        .await
                        .map_err(|err| TlsError::Handshake(err.to_string()))?;
                    return serve_async_connection(tls_stream, server, worker_limit).await;
                }
                Some(AuthTlsRequest::Invalid) => {
                    write_rpc_message(&mut stream, &auth_tls_badcred_reply(message.xid)).await?;
                    continue;
                }
                None => {
                    if matches!(mode, TlsMode::Required | TlsMode::MutualAuthRequired) {
                        write_rpc_message(&mut stream, &auth_tls_badcred_reply(message.xid))
                            .await?;
                        return Ok(());
                    }
                    let (reader, writer) = tokio::io::split(stream);
                    return serve_async_split(
                        reader,
                        writer,
                        server,
                        worker_limit,
                        Some(message),
                        buffer,
                    )
                    .await;
                }
            }
        }

        let read = stream
            .read_buf(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        if read == 0 {
            return Ok(());
        }
    }
}

enum AuthTlsRequest {
    Probe,
    Invalid,
}

fn auth_tls_request(message: &RpcMessage) -> Option<AuthTlsRequest> {
    let MessageBody::Call(call) = &message.body else {
        return None;
    };
    if call.credentials.flavor != AuthFlavor::Unknown(AUTH_TLS_VALUE) {
        return None;
    }
    if call.procedure == Procedure(0)
        && call.credentials.body.is_empty()
        && call.verifier == OpaqueAuth::none()
    {
        Some(AuthTlsRequest::Probe)
    } else {
        Some(AuthTlsRequest::Invalid)
    }
}

fn starttls_ok_reply(xid: Xid) -> Result<RpcMessage, RuntimeError> {
    Ok(RpcMessage {
        xid,
        body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier: OpaqueAuth::new(AuthFlavor::None, Bytes::from_static(STARTTLS_TOKEN))
                .map_err(RuntimeError::Wire)?,
            status: AcceptedStatus::Success(Bytes::new()),
        })),
    })
}

fn auth_tls_badcred_reply(xid: Xid) -> RpcMessage {
    RpcMessage {
        xid,
        body: MessageBody::Reply(ReplyBody::Denied(RejectedReply::AuthError(
            AuthStat::BadCred,
        ))),
    }
}

fn is_tls_close_without_notify(message: &str) -> bool {
    message.contains("close_notify")
}

async fn run_sync_connection_on_runtime<S>(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    stream: S,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ = tx.send(serve_sync_connection(stream, server, worker_limit).await);
    });
    rx.await.map_err(|_| {
        TlsError::ServerTransport(ServerTransportError::Io("server runtime terminated".into()))
    })?
}

async fn run_async_connection_on_runtime<S>(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    stream: S,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ = tx.send(serve_async_connection(stream, server, worker_limit).await);
    });
    rx.await.map_err(|_| {
        TlsError::ServerTransport(ServerTransportError::Io("server runtime terminated".into()))
    })?
}

async fn run_async_starttls_connection_on_runtime(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    stream: TcpStream,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
    acceptor: TlsAcceptor,
    mode: TlsMode,
) -> Result<(), TlsError> {
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ = tx.send(
            serve_starttls_async_connection(stream, server, worker_limit, acceptor, mode).await,
        );
    });
    rx.await.map_err(|_| {
        TlsError::ServerTransport(ServerTransportError::Io("server runtime terminated".into()))
    })?
}

fn build_owned_runtime() -> Result<tokio::runtime::Runtime, TlsError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| TlsError::Handshake(err.to_string()))
}

fn build_server_runtime(
    config: &onc_rpc_server::ServerConfig,
) -> Result<tokio::runtime::Runtime, TlsError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.selector_threads.max(1))
        .thread_name("onc-rpc-tls-server")
        .enable_all()
        .build()
        .map_err(|err| TlsError::Handshake(err.to_string()))
}

fn worker_threads_permits(worker_threads: usize) -> usize {
    worker_threads.max(1)
}

async fn acquire_worker_permit(
    worker_limit: Arc<Semaphore>,
) -> Result<OwnedSemaphorePermit, TlsError> {
    worker_limit.acquire_owned().await.map_err(|_| {
        TlsError::ServerTransport(ServerTransportError::Io(
            "server worker semaphore closed".into(),
        ))
    })
}

struct OwnedRuntime {
    runtime: StdMutex<Option<tokio::runtime::Runtime>>,
}

impl OwnedRuntime {
    fn new(runtime: tokio::runtime::Runtime) -> Self {
        Self {
            runtime: StdMutex::new(Some(runtime)),
        }
    }
}

impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.lock().expect("runtime mutex poisoned").take() {
            let _ = std::thread::spawn(move || drop(runtime)).join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onc_rpc_runtime::{AsyncClient, Client};
    use onc_rpc_server::{
        AsyncDispatch, Dispatch, DispatchError, Program, RequestContext, ResponsePayload,
        ServerBuilder,
    };
    use onc_rpc_xdr::{XdrDecode, XdrEncode};
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::task::JoinHandle;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct EchoValue(u32);

    impl XdrEncode for EchoValue {
        fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), onc_rpc_xdr::XdrError> {
            self.0.encode_xdr(output)
        }
    }

    impl XdrDecode for EchoValue {
        fn decode_xdr(input: &mut &[u8]) -> Result<Self, onc_rpc_xdr::XdrError> {
            Ok(Self(u32::decode_xdr(input)?))
        }
    }

    fn test_program() -> ProgramVersion {
        ProgramVersion {
            program: 700_001,
            version: 1,
        }
    }

    fn test_procedure() -> Procedure {
        Procedure(1)
    }

    struct SyncEcho;

    impl Dispatch for SyncEcho {
        fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
            Ok(ResponsePayload::success(request.payload))
        }
    }

    struct AsyncEcho;

    #[async_trait]
    impl AsyncDispatch for AsyncEcho {
        async fn dispatch(
            &self,
            request: RequestContext,
        ) -> Result<ResponsePayload, DispatchError> {
            Ok(ResponsePayload::success(request.payload))
        }
    }

    fn tls_configs() -> (ClientTlsConfig, ServerTlsConfig) {
        let cert = generate_simple_self_signed(vec!["localhost".into()]).expect("cert");
        let cert_der: CertificateDer<'static> = cert.cert.der().clone();
        let key_der =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));

        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert_der.clone()).expect("root add");

        let client = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .expect("server config");

        (
            ClientTlsConfig::new(
                client,
                ServerName::try_from("localhost").expect("servername"),
            ),
            ServerTlsConfig::new(server),
        )
    }

    async fn setup_client(
        config: ClientConfig,
        tls: ClientTlsConfig,
        starttls: bool,
    ) -> Result<(), TlsError> {
        if starttls {
            TokioAsyncStartTlsClientTransport::connect(&config, &tls, test_program())
                .await
                .map(|_| ())
        } else {
            TokioAsyncTlsClientTransport::connect(&config, &tls)
                .await
                .map(|_| ())
        }
    }

    async fn receive_probe(stream: &mut TcpStream) -> Xid {
        let request = onc_rpc_runtime::read_rpc_message(stream).await.unwrap();
        let MessageBody::Call(call) = request.body else {
            panic!("expected STARTTLS NULL call")
        };
        assert_eq!(call.program, test_program());
        assert_eq!(call.procedure, Procedure(0));
        assert_eq!(call.credentials.flavor.number(), AUTH_TLS_VALUE);
        assert!(call.credentials.body.is_empty());
        assert_eq!(call.verifier, OpaqueAuth::none());
        assert!(call.payload.is_empty());
        request.xid
    }

    fn probe_reply(xid: Xid, verifier: OpaqueAuth, payload: Bytes) -> RpcMessage {
        RpcMessage {
            xid,
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier,
                status: AcceptedStatus::Success(payload),
            })),
        }
    }

    fn starttls_verifier() -> OpaqueAuth {
        OpaqueAuth::new(AuthFlavor::None, Bytes::from_static(STARTTLS_TOKEN)).unwrap()
    }

    async fn accept_probe(stream: &mut TcpStream) {
        let xid = receive_probe(stream).await;
        write_rpc_message(stream, &probe_reply(xid, starttls_verifier(), Bytes::new()))
            .await
            .unwrap();
    }

    async fn receive_client_hello(stream: &mut TcpStream) {
        let mut header = [0_u8; 5];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 22, "expected a TLS handshake record");
        let mut body = vec![0; usize::from(u16::from_be_bytes([header[3], header[4]]))];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(body.first(), Some(&1), "expected ClientHello");
    }

    async fn peer_closed(stream: &mut TcpStream) {
        let mut remaining = Vec::new();
        stream.read_to_end(&mut remaining).await.unwrap();
    }

    #[tokio::test]
    async fn tls_and_starttls_silent_handshake_expire_and_release_socket() {
        for starttls in [false, true] {
            let (client_tls, _) = tls_configs();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = ClientConfig::new(listener.local_addr().unwrap())
                .with_connect_timeout(Duration::from_secs(60));
            let (entered, received) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                if starttls {
                    accept_probe(&mut stream).await;
                }
                receive_client_hello(&mut stream).await;
                entered.send(()).unwrap();
                peer_closed(&mut stream).await;
            });
            let pending = tokio::spawn(setup_client(config, client_tls, starttls));
            timeout(Duration::from_secs(5), received)
                .await
                .unwrap()
                .unwrap();
            // Establish real TCP/ClientHello readiness before pausing time.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(60)).await;
            let result = timeout(Duration::from_secs(1), pending)
                .await
                .expect("handshake exceeded the aggregate setup budget")
                .unwrap();
            tokio::time::resume();
            assert!(
                matches!(result, Err(TlsError::Runtime(RuntimeError::Transport(detail)))
                if detail.contains("TLS setup timeout"))
            );
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn silent_starttls_probe_expires_without_sending_client_hello() {
        let (client_tls, _) = tls_configs();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(listener.local_addr().unwrap())
            .with_connect_timeout(Duration::from_secs(60));
        let (entered, received) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            receive_probe(&mut stream).await;
            entered.send(()).unwrap();
            let mut next = [0_u8; 1];
            assert_eq!(stream.read(&mut next).await.unwrap(), 0);
        });
        let pending = tokio::spawn(setup_client(config, client_tls, true));
        timeout(Duration::from_secs(5), received)
            .await
            .unwrap()
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(60)).await;
        let result = timeout(Duration::from_secs(1), pending)
            .await
            .expect("silent probe exceeded the setup budget")
            .unwrap();
        tokio::time::resume();
        assert!(
            matches!(result, Err(TlsError::Runtime(RuntimeError::Transport(detail)))
            if detail.contains("TLS setup timeout"))
        );
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn starttls_probe_and_handshake_share_one_setup_deadline() {
        let (client_tls, _) = tls_configs();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(listener.local_addr().unwrap())
            .with_connect_timeout(Duration::from_secs(60));
        let (entered, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (hello, hello_received) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let xid = receive_probe(&mut stream).await;
            entered.send(()).unwrap();
            released.await.unwrap();
            write_rpc_message(
                &mut stream,
                &probe_reply(xid, starttls_verifier(), Bytes::new()),
            )
            .await
            .unwrap();
            receive_client_hello(&mut stream).await;
            hello.send(()).unwrap();
            peer_closed(&mut stream).await;
        });
        let pending = tokio::spawn(setup_client(config, client_tls, true));
        timeout(Duration::from_secs(5), received)
            .await
            .unwrap()
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(40)).await;
        tokio::time::resume();
        release.send(()).unwrap();
        timeout(Duration::from_secs(5), hello_received)
            .await
            .unwrap()
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(20)).await;
        let result = timeout(Duration::from_secs(1), pending)
            .await
            .expect("TLS handshake incorrectly received a new setup budget")
            .unwrap();
        tokio::time::resume();
        assert!(
            matches!(result, Err(TlsError::Runtime(RuntimeError::Transport(detail)))
            if detail.contains("TLS setup timeout"))
        );
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn malformed_starttls_replies_never_reach_tls_handshake() {
        let (client_tls, _) = tls_configs();
        for (xid, verifier, payload) in [
            (Xid(2), starttls_verifier(), Bytes::new()),
            (
                Xid(1),
                OpaqueAuth::new(AuthFlavor::Unknown(99), Bytes::from_static(STARTTLS_TOKEN))
                    .unwrap(),
                Bytes::new(),
            ),
            (
                Xid(1),
                OpaqueAuth::new(AuthFlavor::None, Bytes::from_static(b"STARTTLX")).unwrap(),
                Bytes::new(),
            ),
            (
                Xid(1),
                starttls_verifier(),
                Bytes::from_static(&[0, 0, 0, 0]),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = ClientConfig::new(listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(receive_probe(&mut stream).await, Xid(1));
                write_rpc_message(&mut stream, &probe_reply(xid, verifier, payload))
                    .await
                    .unwrap();
                let mut next = [0_u8; 1];
                assert_eq!(
                    stream.read(&mut next).await.unwrap(),
                    0,
                    "invalid STARTTLS reply was followed by TLS traffic"
                );
            });
            assert!(matches!(
                timeout(
                    Duration::from_secs(5),
                    setup_client(config, client_tls.clone(), true)
                )
                .await
                .unwrap(),
                Err(TlsError::InvalidStartTlsReply)
            ));
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn oversized_or_excessively_fragmented_probe_fails_without_payload_wait() {
        let (client_tls, _) = tls_configs();
        let oversized = (0x8000_0000_u32 | STARTTLS_PROBE_LIMITS.max_record_bytes as u32)
            .to_be_bytes()
            .to_vec();
        let fragments = vec![0_u8; STARTTLS_PROBE_LIMITS.max_fragments * 4];
        for record in [oversized, fragments] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = ClientConfig::new(listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                receive_probe(&mut stream).await;
                stream.write_all(&record).await.unwrap();
                let mut next = [0_u8; 1];
                assert_eq!(stream.read(&mut next).await.unwrap(), 0);
            });
            assert!(matches!(
                timeout(
                    Duration::from_secs(5),
                    setup_client(config, client_tls.clone(), true)
                )
                .await
                .unwrap(),
                Err(TlsError::Runtime(RuntimeError::Transport(_)))
            ));
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn fragmented_valid_probe_enters_strict_sunrpc_tls() {
        let (client_tls, server_tls) = tls_configs();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let xid = receive_probe(&mut stream).await;
            let payload = probe_reply(xid, starttls_verifier(), Bytes::new())
                .encode()
                .unwrap();
            stream.write_all(&4_u32.to_be_bytes()).await.unwrap();
            stream.write_all(&payload[..4]).await.unwrap();
            stream
                .write_all(&(0x8000_0000_u32 | (payload.len() - 4) as u32).to_be_bytes())
                .await
                .unwrap();
            stream.write_all(&payload[4..]).await.unwrap();
            let tls = TlsAcceptor::from(server_tls.tls_config)
                .accept(stream)
                .await
                .unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(), Some(SUNRPC_ALPN));
        });
        timeout(
            Duration::from_secs(5),
            setup_client(config, client_tls, true),
        )
        .await
        .unwrap()
        .unwrap();
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn tls_and_starttls_still_reject_absent_sunrpc_alpn() {
        for starttls in [false, true] {
            let (client_tls, server_tls) = tls_configs();
            let mut server_config = (*server_tls.tls_config).clone();
            server_config.alpn_protocols.clear();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = ClientConfig::new(listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                if starttls {
                    accept_probe(&mut stream).await;
                }
                let tls = TlsAcceptor::from(Arc::new(server_config))
                    .accept(stream)
                    .await
                    .unwrap();
                assert!(tls.get_ref().1.alpn_protocol().is_none());
            });
            assert!(matches!(
                timeout(
                    Duration::from_secs(5),
                    setup_client(config, client_tls, starttls)
                )
                .await
                .unwrap(),
                Err(TlsError::MissingSunrpcAlpn)
            ));
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_tls_transport_round_trips() {
        let (client_tls, server_tls) = tls_configs();
        let mut server = ServerBuilder::new()
            .with_bind_addr("127.0.0.1:0".parse().expect("addr"))
            .build_async();
        server
            .register(
                Program {
                    number: test_program().program,
                    version: test_program().version,
                },
                AsyncEcho,
            )
            .expect("register");
        let transport = TokioAsyncTlsServerTransport::bind(server, server_tls)
            .await
            .expect("bind");
        let addr = transport.local_addr().expect("local addr");
        let task: JoinHandle<()> = tokio::spawn(async move {
            let _ = transport.serve().await;
        });

        let config = ClientConfig::new(addr);
        let transport = TokioAsyncTlsClientTransport::connect(&config, &client_tls)
            .await
            .expect("connect");
        let client = AsyncClient::new(config, transport);
        let reply: EchoValue = client
            .call_typed(test_program(), test_procedure(), &EchoValue(99))
            .await
            .expect("call");
        assert_eq!(reply, EchoValue(99));

        task.abort();
    }

    #[test]
    fn sync_tls_transport_round_trips() {
        let (client_tls, server_tls) = tls_configs();
        let (addr_tx, addr_rx) = std::sync::mpsc::sync_channel(1);
        let server_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let mut server = ServerBuilder::new()
                    .with_bind_addr("127.0.0.1:0".parse().expect("addr"))
                    .build();
                server
                    .register(
                        Program {
                            number: test_program().program,
                            version: test_program().version,
                        },
                        SyncEcho,
                    )
                    .expect("register");
                let transport = TokioTlsServerTransport::bind(server, server_tls)
                    .await
                    .expect("bind");
                addr_tx
                    .send(transport.local_addr().expect("local"))
                    .expect("send");
                transport.accept_once().await.expect("accept once");
            });
        });
        let addr = addr_rx.recv().expect("recv");
        let config = ClientConfig::new(addr);
        let transport = TokioTlsClientTransport::connect(&config, &client_tls).expect("connect");
        let client = Client::new(config, transport);
        let reply: EchoValue = client
            .call_typed(test_program(), test_procedure(), &EchoValue(7))
            .expect("call");
        assert_eq!(reply, EchoValue(7));
        drop(client);
        server_thread.join().expect("join");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_starttls_transport_round_trips() {
        let (client_tls, server_tls) = tls_configs();
        let mut server = ServerBuilder::new()
            .with_bind_addr("127.0.0.1:0".parse().expect("addr"))
            .build_async();
        server
            .register(
                Program {
                    number: test_program().program,
                    version: test_program().version,
                },
                AsyncEcho,
            )
            .expect("register");
        let transport =
            TokioAsyncStartTlsServerTransport::bind(server, server_tls, TlsMode::Required)
                .await
                .expect("bind");
        let addr = transport.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = transport.serve().await;
        });

        let config = ClientConfig::new(addr);
        let transport =
            TokioAsyncStartTlsClientTransport::connect(&config, &client_tls, test_program())
                .await
                .expect("connect");
        let client = AsyncClient::new(config, transport);
        let reply: EchoValue = client
            .call_typed(test_program(), test_procedure(), &EchoValue(123))
            .await
            .expect("call");
        assert_eq!(reply, EchoValue(123));

        task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn starttls_rejects_invalid_probe() {
        let (_client_tls, server_tls) = tls_configs();
        let mut server = ServerBuilder::new()
            .with_bind_addr("127.0.0.1:0".parse().expect("addr"))
            .build_async();
        server
            .register(
                Program {
                    number: test_program().program,
                    version: test_program().version,
                },
                AsyncEcho,
            )
            .expect("register");
        let transport =
            TokioAsyncStartTlsServerTransport::bind(server, server_tls, TlsMode::Required)
                .await
                .expect("bind");
        let addr = transport.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = transport.serve().await;
        });

        let mut stream = TcpStream::connect(addr).await.expect("tcp connect");
        let bad = RpcMessage {
            xid: Xid(77),
            body: MessageBody::Call(onc_rpc_wire::CallBody::new(
                test_program(),
                Procedure(99),
                OpaqueAuth::new(AuthFlavor::Unknown(AUTH_TLS_VALUE), Bytes::new()).expect("auth"),
                OpaqueAuth::none(),
                Bytes::new(),
            )),
        };
        write_rpc_message(&mut stream, &bad).await.expect("write");
        let mut buffer = BytesMut::new();
        loop {
            if let Some(reply) = try_decode_message_from_buffer(&mut buffer).expect("decode") {
                match reply.body {
                    MessageBody::Reply(ReplyBody::Denied(RejectedReply::AuthError(
                        AuthStat::BadCred,
                    ))) => break,
                    other => panic!("unexpected reply: {other:?}"),
                }
            }
            if stream.read_buf(&mut buffer).await.expect("read") == 0 {
                panic!("connection closed before reply");
            }
        }

        task.abort();
    }
}
