use crate::{AsyncServer, Server};
use bytes::BytesMut;
use onc_rpc_runtime::{RuntimeError, WireError, try_decode_message_from_buffer, write_rpc_message};
use std::sync::{Arc, Mutex as StdMutex};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
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
