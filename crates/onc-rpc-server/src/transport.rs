use crate::{AsyncServer, Server};
use bytes::{Bytes, BytesMut};
use onc_rpc_runtime::{
    RuntimeError, WireError, decode_rpc_message_datagram, encode_rpc_message_datagram,
    try_decode_message_from_buffer, write_rpc_message,
};
use std::sync::{Arc, Mutex as StdMutex};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, oneshot};

#[derive(Debug, Error)]
pub enum ServerTransportError {
    #[error("i/o failure: {0}")]
    Io(String),
    #[error("wire failure: {0}")]
    Wire(WireError),
    #[error("server dispatch failure: {0}")]
    Server(#[from] crate::ServerError),
    #[error("runtime transport failure: {0}")]
    Runtime(#[from] RuntimeError),
}

pub struct TokioServerTransport {
    listener: TcpListener,
    server: Arc<Server>,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

pub struct TokioAsyncServerTransport {
    listener: TcpListener,
    server: Arc<AsyncServer>,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

pub struct TokioUdpServerTransport {
    socket: Arc<UdpSocket>,
    server: Arc<Server>,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

pub struct TokioAsyncUdpServerTransport {
    socket: Arc<UdpSocket>,
    server: Arc<AsyncServer>,
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    worker_limit: Arc<Semaphore>,
}

impl TokioServerTransport {
    pub async fn bind(server: Server) -> Result<Self, ServerTransportError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, ServerTransportError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()))
    }

    pub fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    pub fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }

    pub async fn accept_once(&self) -> Result<(), ServerTransportError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        run_sync_connection_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            stream,
            self.server.clone(),
            self.worker_limit.clone(),
        )
        .await
    }

    pub async fn serve(self) -> Result<(), ServerTransportError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            let runtime_guard = self.runtime_guard.clone();
            let worker_limit = self.worker_limit.clone();
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                let _ = serve_sync_connection(stream, server, worker_limit).await;
            });
        }
    }
}

impl crate::ServerTransportIntrospection for TokioServerTransport {
    fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }
}

impl TokioAsyncServerTransport {
    pub async fn bind(server: AsyncServer) -> Result<Self, ServerTransportError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let listener = TcpListener::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            listener,
            server: Arc::new(server),
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, ServerTransportError> {
        self.listener
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()))
    }

    pub fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    pub fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }

    pub async fn accept_once(&self) -> Result<(), ServerTransportError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        run_async_connection_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            stream,
            self.server.clone(),
            self.worker_limit.clone(),
        )
        .await
    }

    pub async fn serve(self) -> Result<(), ServerTransportError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|err| ServerTransportError::Io(err.to_string()))?;
            let server = self.server.clone();
            let runtime_guard = self.runtime_guard.clone();
            let worker_limit = self.worker_limit.clone();
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                let _ = serve_async_connection(stream, server, worker_limit).await;
            });
        }
    }
}

impl crate::AsyncServerTransportIntrospection for TokioAsyncServerTransport {
    fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }
}

impl TokioUdpServerTransport {
    pub async fn bind(server: Server) -> Result<Self, ServerTransportError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let socket = UdpSocket::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            socket: Arc::new(socket),
            server: Arc::new(server),
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, ServerTransportError> {
        self.socket
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()))
    }

    pub fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    pub fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }

    pub async fn accept_once(&self) -> Result<(), ServerTransportError> {
        let (peer, datagram) = recv_udp_datagram(self.socket.clone()).await?;
        run_sync_udp_datagram_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            self.socket.clone(),
            self.server.clone(),
            self.worker_limit.clone(),
            peer,
            datagram,
        )
        .await
    }

    pub async fn serve(self) -> Result<(), ServerTransportError> {
        loop {
            let (peer, datagram) = recv_udp_datagram(self.socket.clone()).await?;
            let socket = self.socket.clone();
            let server = self.server.clone();
            let runtime_guard = self.runtime_guard.clone();
            let worker_limit = self.worker_limit.clone();
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                let _ =
                    handle_sync_udp_datagram(socket, server, worker_limit, peer, datagram).await;
            });
        }
    }
}

impl crate::ServerTransportIntrospection for TokioUdpServerTransport {
    fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }
}

impl TokioAsyncUdpServerTransport {
    pub async fn bind(server: AsyncServer) -> Result<Self, ServerTransportError> {
        let config = server.config().clone();
        let runtime = build_server_runtime(&config)?;
        let socket = UdpSocket::bind(config.bind_addr)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        Ok(Self {
            socket: Arc::new(socket),
            server: Arc::new(server),
            runtime_handle: runtime.handle().clone(),
            runtime_guard: Arc::new(OwnedRuntime::new(runtime)),
            worker_limit: Arc::new(Semaphore::new(worker_threads_permits(
                config.worker_threads,
            ))),
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, ServerTransportError> {
        self.socket
            .local_addr()
            .map_err(|err| ServerTransportError::Io(err.to_string()))
    }

    pub fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    pub fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }

    pub async fn accept_once(&self) -> Result<(), ServerTransportError> {
        let (peer, datagram) = recv_udp_datagram(self.socket.clone()).await?;
        run_async_udp_datagram_on_runtime(
            self.runtime_handle.clone(),
            self.runtime_guard.clone(),
            self.socket.clone(),
            self.server.clone(),
            self.worker_limit.clone(),
            peer,
            datagram,
        )
        .await
    }

    pub async fn serve(self) -> Result<(), ServerTransportError> {
        loop {
            let (peer, datagram) = recv_udp_datagram(self.socket.clone()).await?;
            let socket = self.socket.clone();
            let server = self.server.clone();
            let runtime_guard = self.runtime_guard.clone();
            let worker_limit = self.worker_limit.clone();
            self.runtime_handle.spawn(async move {
                let _runtime_guard = runtime_guard;
                let _ =
                    handle_async_udp_datagram(socket, server, worker_limit, peer, datagram).await;
            });
        }
    }
}

impl crate::AsyncServerTransportIntrospection for TokioAsyncUdpServerTransport {
    fn config(&self) -> &crate::ServerConfig {
        self.server.config()
    }

    fn registered_programs(&self) -> Vec<crate::Program> {
        self.server.registered_programs()
    }
}

async fn serve_sync_connection(
    stream: TcpStream,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), ServerTransportError> {
    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut buffer = BytesMut::with_capacity(8192);

    loop {
        while let Some(message) =
            try_decode_message_from_buffer(&mut buffer).map_err(map_runtime_error)?
        {
            let server = server.clone();
            let writer = writer.clone();
            let permit = acquire_worker_permit(worker_limit.clone()).await?;
            tokio::spawn(async move {
                let _permit = permit;
                if let Ok(reply) = server.handle_message(message) {
                    let mut writer = writer.lock().await;
                    let _ = write_rpc_message(&mut *writer, &reply).await;
                }
            });
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        if read == 0 {
            return Ok(());
        }
    }
}

async fn serve_async_connection(
    stream: TcpStream,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), ServerTransportError> {
    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut buffer = BytesMut::with_capacity(8192);

    loop {
        while let Some(message) =
            try_decode_message_from_buffer(&mut buffer).map_err(map_runtime_error)?
        {
            let server = server.clone();
            let writer = writer.clone();
            let permit = acquire_worker_permit(worker_limit.clone()).await?;
            tokio::spawn(async move {
                let _permit = permit;
                if let Ok(reply) = server.handle_message(message).await {
                    let mut writer = writer.lock().await;
                    let _ = write_rpc_message(&mut *writer, &reply).await;
                }
            });
        }

        let read = reader
            .read_buf(&mut buffer)
            .await
            .map_err(|err| ServerTransportError::Io(err.to_string()))?;
        if read == 0 {
            return Ok(());
        }
    }
}

async fn recv_udp_datagram(
    socket: Arc<UdpSocket>,
) -> Result<(std::net::SocketAddr, Bytes), ServerTransportError> {
    let mut buffer = vec![0_u8; 65_536];
    let (read, peer) = socket
        .recv_from(&mut buffer)
        .await
        .map_err(|err| ServerTransportError::Io(err.to_string()))?;
    Ok((peer, Bytes::copy_from_slice(&buffer[..read])))
}

async fn handle_sync_udp_datagram(
    socket: Arc<UdpSocket>,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
    peer: std::net::SocketAddr,
    datagram: Bytes,
) -> Result<(), ServerTransportError> {
    let permit = acquire_worker_permit(worker_limit).await?;
    let _permit = permit;
    let message = decode_rpc_message_datagram(&datagram).map_err(map_runtime_error)?;
    let reply = server.handle_message(message)?;
    let payload = encode_rpc_message_datagram(&reply)?;
    socket
        .send_to(&payload, peer)
        .await
        .map_err(|err| ServerTransportError::Io(err.to_string()))?;
    Ok(())
}

async fn handle_async_udp_datagram(
    socket: Arc<UdpSocket>,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
    peer: std::net::SocketAddr,
    datagram: Bytes,
) -> Result<(), ServerTransportError> {
    let permit = acquire_worker_permit(worker_limit).await?;
    let _permit = permit;
    let message = decode_rpc_message_datagram(&datagram).map_err(map_runtime_error)?;
    let reply = server.handle_message(message).await?;
    let payload = encode_rpc_message_datagram(&reply)?;
    socket
        .send_to(&payload, peer)
        .await
        .map_err(|err| ServerTransportError::Io(err.to_string()))?;
    Ok(())
}

fn map_runtime_error(error: RuntimeError) -> ServerTransportError {
    match error {
        RuntimeError::Wire(err) => ServerTransportError::Wire(err),
        other => ServerTransportError::Runtime(other),
    }
}

async fn run_sync_connection_on_runtime(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    stream: TcpStream,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), ServerTransportError> {
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ = tx.send(serve_sync_connection(stream, server, worker_limit).await);
    });
    rx.await
        .map_err(|_| ServerTransportError::Io("server runtime terminated".into()))?
}

async fn run_async_connection_on_runtime(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    stream: TcpStream,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
) -> Result<(), ServerTransportError> {
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ = tx.send(serve_async_connection(stream, server, worker_limit).await);
    });
    rx.await
        .map_err(|_| ServerTransportError::Io("server runtime terminated".into()))?
}

async fn run_sync_udp_datagram_on_runtime(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    socket: Arc<UdpSocket>,
    server: Arc<Server>,
    worker_limit: Arc<Semaphore>,
    peer: std::net::SocketAddr,
    datagram: Bytes,
) -> Result<(), ServerTransportError> {
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ =
            tx.send(handle_sync_udp_datagram(socket, server, worker_limit, peer, datagram).await);
    });
    rx.await
        .map_err(|_| ServerTransportError::Io("server runtime terminated".into()))?
}

async fn run_async_udp_datagram_on_runtime(
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    socket: Arc<UdpSocket>,
    server: Arc<AsyncServer>,
    worker_limit: Arc<Semaphore>,
    peer: std::net::SocketAddr,
    datagram: Bytes,
) -> Result<(), ServerTransportError> {
    let (tx, rx) = oneshot::channel();
    runtime_handle.spawn(async move {
        let _runtime_guard = runtime_guard;
        let _ =
            tx.send(handle_async_udp_datagram(socket, server, worker_limit, peer, datagram).await);
    });
    rx.await
        .map_err(|_| ServerTransportError::Io("server runtime terminated".into()))?
}

fn build_server_runtime(
    config: &crate::ServerConfig,
) -> Result<tokio::runtime::Runtime, ServerTransportError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.selector_threads.max(1))
        .thread_name("onc-rpc-server")
        .enable_all()
        .build()
        .map_err(|err| ServerTransportError::Io(err.to_string()))
}

fn worker_threads_permits(worker_threads: usize) -> usize {
    worker_threads.max(1)
}

async fn acquire_worker_permit(
    worker_limit: Arc<Semaphore>,
) -> Result<OwnedSemaphorePermit, ServerTransportError> {
    worker_limit
        .acquire_owned()
        .await
        .map_err(|_| ServerTransportError::Io("server worker semaphore closed".into()))
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
