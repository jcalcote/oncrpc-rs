use crate::{
    AsyncClientTransport, ClientConfig, ClientTransport, RpcMessage, RuntimeError, Xid,
    try_decode_message_from_buffer, write_rpc_message,
};
use async_trait::async_trait;
use bytes::BytesMut;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpSocket, TcpStream, tcp::OwnedReadHalf, tcp::OwnedWriteHalf};
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;

type PendingMap = Arc<Mutex<HashMap<Xid, oneshot::Sender<Result<RpcMessage, RuntimeError>>>>>;

#[derive(Clone)]
pub struct TokioAsyncClientTransport {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    pending: PendingMap,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
}

pub struct TokioClientTransport {
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: TokioAsyncClientTransport,
}

impl TokioAsyncClientTransport {
    pub async fn connect(config: &ClientConfig) -> Result<Self, RuntimeError> {
        let socket = socket_for_remote(config.remote_addr)?;
        if let Some(local_addr) = config.local_addr {
            socket
                .bind(local_addr)
                .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        }

        let stream = timeout(config.connect_timeout, socket.connect(config.remote_addr))
            .await
            .map_err(|_| {
                RuntimeError::Transport(format!(
                    "connect timeout after {:?}",
                    config.connect_timeout
                ))
            })?
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;

        Self::from_stream_with_timeouts(stream, config.default_call_timeout, config.write_timeout)
    }
    fn from_stream_with_timeouts(
        stream: TcpStream,
        default_call_timeout: Option<std::time::Duration>,
        write_timeout: Option<std::time::Duration>,
    ) -> Result<Self, RuntimeError> {
        let (reader, writer) = stream.into_split();
        let pending = Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(reader_task(reader, pending.clone()));

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            pending,
            default_call_timeout,
            write_timeout,
        })
    }
}

impl TokioClientTransport {
    pub fn connect(config: &ClientConfig) -> Result<Self, RuntimeError> {
        let runtime = build_owned_runtime()?;
        let runtime_handle = runtime.handle().clone();
        let runtime_guard = Arc::new(OwnedRuntime::new(runtime));
        let inner = runtime_handle.block_on(TokioAsyncClientTransport::connect(config))?;

        Ok(Self {
            runtime_handle,
            runtime_guard,
            inner,
        })
    }
}

fn build_owned_runtime() -> Result<tokio::runtime::Runtime, RuntimeError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| RuntimeError::Transport(err.to_string()))
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

#[async_trait]
impl AsyncClientTransport for TokioAsyncClientTransport {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        let xid = request.xid;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(xid, tx);

        let write_future = async {
            let mut writer = self.writer.lock().await;
            write_rpc_message(&mut *writer, &request).await
        };
        let write_result = match self.write_timeout {
            Some(timeout_duration) => {
                timeout(timeout_duration, write_future).await.map_err(|_| {
                    RuntimeError::Transport(format!("write timeout after {:?}", timeout_duration))
                })?
            }
            None => write_future.await,
        };

        if let Err(error) = write_result {
            self.pending.lock().await.remove(&xid);
            return Err(error);
        }

        let reply_result = match self.default_call_timeout {
            Some(timeout_duration) => match timeout(timeout_duration, rx).await {
                Ok(result) => result,
                Err(_) => {
                    self.pending.lock().await.remove(&xid);
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
}

impl ClientTransport for TokioClientTransport {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        let (tx, rx) = mpsc::sync_channel(1);
        let inner = self.inner.clone();
        let _runtime_guard = self.runtime_guard.clone();

        self.runtime_handle.spawn(async move {
            let _ = tx.send(inner.call(request).await);
        });

        rx.recv().map_err(|_| RuntimeError::ConnectionClosed)?
    }
}

async fn reader_task(mut reader: OwnedReadHalf, pending: PendingMap) {
    let result = reader_loop(&mut reader, pending.clone()).await;
    if let Err(error) = result {
        let mut pending = pending.lock().await;
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(error.clone()));
        }
    }
}

async fn reader_loop(reader: &mut OwnedReadHalf, pending: PendingMap) -> Result<(), RuntimeError> {
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

fn socket_for_remote(remote_addr: std::net::SocketAddr) -> Result<TcpSocket, RuntimeError> {
    match remote_addr {
        std::net::SocketAddr::V4(_) => {
            TcpSocket::new_v4().map_err(|err| RuntimeError::Transport(err.to_string()))
        }
        std::net::SocketAddr::V6(_) => {
            TcpSocket::new_v6().map_err(|err| RuntimeError::Transport(err.to_string()))
        }
    }
}
