//! TLS and STARTTLS integration points for ONC RPC.

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClientTransport, AuthStat, CallOptions, ClientConfig,
    ClientTransport, MessageBody, OpaqueAuth, Procedure, ProgramVersion, RejectedReply, ReplyBody,
    RpcMessage, RuntimeError, Xid, try_decode_message_from_buffer, write_rpc_message,
};
use onc_rpc_server::{AsyncServer, Server, ServerError, ServerTransportError};
use onc_rpc_wire::AuthFlavor;
use rustls::pki_types::ServerName;
use std::collections::HashMap;
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

type PendingMap = Arc<Mutex<HashMap<Xid, oneshot::Sender<Result<RpcMessage, RuntimeError>>>>>;

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
    writer: Arc<Mutex<tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>>>,
    pending: PendingMap,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
}

pub struct TokioTlsClientTransport {
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: TokioAsyncTlsClientTransport,
}

#[derive(Clone)]
pub struct TokioAsyncStartTlsClientTransport {
    writer: Arc<Mutex<tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>>>,
    pending: PendingMap,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
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
    pub async fn connect(config: &ClientConfig, tls: &ClientTlsConfig) -> Result<Self, TlsError> {
        let stream = tcp_connect(config).await?;
        let connector = TlsConnector::from(tls.tls_config.clone());
        let tls_stream = connector
            .connect(tls.server_name.clone(), stream)
            .await
            .map_err(|err| TlsError::Handshake(err.to_string()))?;
        validate_client_alpn(&tls_stream)?;
        Ok(from_tls_stream(
            tls_stream,
            config.default_call_timeout,
            config.write_timeout,
        ))
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
    pub async fn connect(
        config: &ClientConfig,
        tls: &ClientTlsConfig,
        program: ProgramVersion,
    ) -> Result<Self, TlsError> {
        let mut stream = tcp_connect(config).await?;
        perform_starttls_probe(&mut stream, program).await?;
        let connector = TlsConnector::from(tls.tls_config.clone());
        let tls_stream = connector
            .connect(tls.server_name.clone(), stream)
            .await
            .map_err(|err| TlsError::Handshake(err.to_string()))?;
        validate_client_alpn(&tls_stream)?;
        Ok(from_starttls_stream(
            tls_stream,
            config.default_call_timeout,
            config.write_timeout,
        ))
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
        async_call(
            self.writer.clone(),
            self.pending.clone(),
            self.write_timeout,
            self.default_call_timeout,
            request,
            &CallOptions::default(),
        )
        .await
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        async_call(
            self.writer.clone(),
            self.pending.clone(),
            self.write_timeout,
            self.default_call_timeout,
            request,
            options,
        )
        .await
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
        async_call(
            self.writer.clone(),
            self.pending.clone(),
            self.write_timeout,
            self.default_call_timeout,
            request,
            &CallOptions::default(),
        )
        .await
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        async_call(
            self.writer.clone(),
            self.pending.clone(),
            self.write_timeout,
            self.default_call_timeout,
            request,
            options,
        )
        .await
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

fn from_tls_stream(
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
) -> TokioAsyncTlsClientTransport {
    let (reader, writer) = tokio::io::split(stream);
    let pending = Arc::new(Mutex::new(HashMap::new()));
    tokio::spawn(reader_task(reader, pending.clone()));
    TokioAsyncTlsClientTransport {
        writer: Arc::new(Mutex::new(writer)),
        pending,
        default_call_timeout,
        write_timeout,
    }
}

fn from_starttls_stream(
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
) -> TokioAsyncStartTlsClientTransport {
    let (reader, writer) = tokio::io::split(stream);
    let pending = Arc::new(Mutex::new(HashMap::new()));
    tokio::spawn(reader_task(reader, pending.clone()));
    TokioAsyncStartTlsClientTransport {
        writer: Arc::new(Mutex::new(writer)),
        pending,
        default_call_timeout,
        write_timeout,
    }
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

    let mut buffer = BytesMut::with_capacity(1024);
    loop {
        if let Some(reply) = try_decode_message_from_buffer(&mut buffer)? {
            return validate_starttls_reply(reply);
        }
        let read = stream
            .read_buf(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        if read == 0 {
            return Err(TlsError::StartTlsRejected);
        }
    }
}

fn validate_starttls_reply(reply: RpcMessage) -> Result<(), TlsError> {
    match reply.body {
        MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
            verifier,
            status: AcceptedStatus::Success(_),
        })) if verifier.flavor == AuthFlavor::None && verifier.body.as_ref() == STARTTLS_TOKEN => {
            Ok(())
        }
        MessageBody::Reply(ReplyBody::Denied(RejectedReply::AuthError(_))) => {
            Err(TlsError::StartTlsRejected)
        }
        _ => Err(TlsError::InvalidStartTlsReply),
    }
}

async fn async_call<W>(
    writer: Arc<Mutex<W>>,
    pending: PendingMap,
    write_timeout: Option<std::time::Duration>,
    default_call_timeout: Option<std::time::Duration>,
    request: RpcMessage,
    options: &CallOptions,
) -> Result<RpcMessage, RuntimeError>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let xid = request.xid;
    let (tx, rx) = oneshot::channel();
    pending.lock().await.insert(xid, tx);

    let write_future = async {
        let mut writer = writer.lock().await;
        write_rpc_message(&mut *writer, &request).await
    };
    let write_result = match write_timeout {
        Some(timeout_duration) => timeout(timeout_duration, write_future).await.map_err(|_| {
            RuntimeError::Transport(format!("write timeout after {:?}", timeout_duration))
        })?,
        None => write_future.await,
    };

    if let Err(error) = write_result {
        pending.lock().await.remove(&xid);
        return Err(error);
    }

    let reply_result = match options.effective_timeout(default_call_timeout) {
        Some(timeout_duration) => match timeout(timeout_duration, rx).await {
            Ok(result) => result,
            Err(_) => {
                pending.lock().await.remove(&xid);
                return Err(RuntimeError::Transport(format!(
                    "call timeout after {:?}",
                    timeout_duration
                )));
            }
        },
        None => rx.await,
    };

    match reply_result {
        Ok(result) => result,
        Err(_) => Err(RuntimeError::ConnectionClosed),
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

async fn reader_task<R>(mut reader: R, pending: PendingMap)
where
    R: AsyncRead + Unpin,
{
    let result = reader_loop(&mut reader, pending.clone()).await;
    if let Err(error) = result {
        let mut pending = pending.lock().await;
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(error.clone()));
        }
    }
}

async fn reader_loop<R>(reader: &mut R, pending: PendingMap) -> Result<(), RuntimeError>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = BytesMut::with_capacity(8192);

    loop {
        while let Some(message) = try_decode_message_from_buffer(&mut buffer)? {
            let xid = message.xid;
            if let Some(sender) = pending.lock().await.remove(&xid) {
                let _ = sender.send(Ok(message));
            }
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        if read == 0 {
            return Err(RuntimeError::ConnectionClosed);
        }
    }
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
