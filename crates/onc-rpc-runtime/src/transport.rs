use crate::{
    AsyncClientTransport, CallOptions, ClientConfig, ClientTransport, RpcMessage, RuntimeError,
    Xid, decode_rpc_message_datagram, encode_rpc_message_datagram, try_decode_message_from_buffer,
    write_rpc_message,
};
use async_trait::async_trait;
use bytes::BytesMut;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpSocket, TcpStream, UdpSocket, tcp::OwnedReadHalf, tcp::OwnedWriteHalf};
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

#[derive(Clone)]
pub struct TokioAsyncUdpClientTransport {
    socket: Arc<UdpSocket>,
    pending: PendingMap,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
}

pub struct TokioClientTransport {
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: TokioAsyncClientTransport,
}

pub struct TokioUdpClientTransport {
    runtime_handle: tokio::runtime::Handle,
    runtime_guard: Arc<OwnedRuntime>,
    inner: TokioAsyncUdpClientTransport,
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

impl TokioAsyncUdpClientTransport {
    pub async fn connect(config: &ClientConfig) -> Result<Self, RuntimeError> {
        let bind_addr = config
            .local_addr
            .unwrap_or_else(|| udp_unspecified_addr(config.remote_addr));
        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;

        timeout(config.connect_timeout, socket.connect(config.remote_addr))
            .await
            .map_err(|_| {
                RuntimeError::Transport(format!(
                    "connect timeout after {:?}",
                    config.connect_timeout
                ))
            })?
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;

        Self::from_socket_with_timeouts(socket, config.default_call_timeout, config.write_timeout)
    }

    fn from_socket_with_timeouts(
        socket: UdpSocket,
        default_call_timeout: Option<std::time::Duration>,
        write_timeout: Option<std::time::Duration>,
    ) -> Result<Self, RuntimeError> {
        let socket = Arc::new(socket);
        let pending = Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(udp_reader_task(socket.clone(), pending.clone()));

        Ok(Self {
            socket,
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

impl TokioUdpClientTransport {
    pub fn connect(config: &ClientConfig) -> Result<Self, RuntimeError> {
        let runtime = build_owned_runtime()?;
        let runtime_handle = runtime.handle().clone();
        let runtime_guard = Arc::new(OwnedRuntime::new(runtime));
        let inner = runtime_handle.block_on(TokioAsyncUdpClientTransport::connect(config))?;

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
        self.call_with_options(request, &CallOptions::default())
            .await
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
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

        await_reply(&self.pending, xid, rx, options, self.default_call_timeout).await
    }
}

#[async_trait]
impl AsyncClientTransport for TokioAsyncUdpClientTransport {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
            .await
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        let xid = request.xid;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(xid, tx);

        let payload = encode_rpc_message_datagram(&request)?;
        if let Err(error) = send_udp_request(&self.socket, &payload, self.write_timeout).await {
            self.pending.lock().await.remove(&xid);
            return Err(error);
        }

        await_reply(&self.pending, xid, rx, options, self.default_call_timeout).await
    }
}

impl ClientTransport for TokioClientTransport {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
    }

    fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        let (tx, rx) = mpsc::sync_channel(1);
        let inner = self.inner.clone();
        let _runtime_guard = self.runtime_guard.clone();
        let options = options.clone();

        self.runtime_handle.spawn(async move {
            let _ = tx.send(inner.call_with_options(request, &options).await);
        });

        rx.recv().map_err(|_| RuntimeError::ConnectionClosed)?
    }
}

impl ClientTransport for TokioUdpClientTransport {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
    }

    fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        let (tx, rx) = mpsc::sync_channel(1);
        let inner = self.inner.clone();
        let _runtime_guard = self.runtime_guard.clone();
        let options = options.clone();

        self.runtime_handle.spawn(async move {
            let _ = tx.send(inner.call_with_options(request, &options).await);
        });

        rx.recv().map_err(|_| RuntimeError::ConnectionClosed)?
    }
}

async fn await_reply(
    pending: &PendingMap,
    xid: Xid,
    rx: oneshot::Receiver<Result<RpcMessage, RuntimeError>>,
    options: &CallOptions,
    default_call_timeout: Option<std::time::Duration>,
) -> Result<RpcMessage, RuntimeError> {
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

async fn send_udp_request(
    socket: &Arc<UdpSocket>,
    payload: &bytes::Bytes,
    write_timeout: Option<std::time::Duration>,
) -> Result<(), RuntimeError> {
    let write_future = socket.send(payload);
    let write_result = match write_timeout {
        Some(timeout_duration) => timeout(timeout_duration, write_future).await.map_err(|_| {
            RuntimeError::Transport(format!("write timeout after {:?}", timeout_duration))
        })?,
        None => write_future.await,
    };

    write_result
        .map(|_| ())
        .map_err(|error| RuntimeError::Transport(error.to_string()))
}

async fn reader_task(mut reader: OwnedReadHalf, pending: PendingMap) {
    let result = reader_loop(&mut reader, pending.clone()).await;
    if let Err(error) = result {
        fail_pending(&pending, error).await;
    }
}

async fn udp_reader_task(socket: Arc<UdpSocket>, pending: PendingMap) {
    let result = udp_reader_loop(socket, pending.clone()).await;
    if let Err(error) = result {
        fail_pending(&pending, error).await;
    }
}

async fn fail_pending(pending: &PendingMap, error: RuntimeError) {
    let mut pending = pending.lock().await;
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(error.clone()));
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

async fn udp_reader_loop(socket: Arc<UdpSocket>, pending: PendingMap) -> Result<(), RuntimeError> {
    let mut buffer = vec![0_u8; 65_536];

    loop {
        let read = socket
            .recv(&mut buffer)
            .await
            .map_err(|err| RuntimeError::Transport(err.to_string()))?;
        if read == 0 {
            continue;
        }

        let message = match decode_rpc_message_datagram(&buffer[..read]) {
            Ok(message) => message,
            Err(RuntimeError::Wire(_)) => continue,
            Err(error) => return Err(error),
        };
        let xid = message.xid;
        if let Some(sender) = pending.lock().await.remove(&xid) {
            let _ = sender.send(Ok(message));
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

fn udp_unspecified_addr(remote_addr: SocketAddr) -> SocketAddr {
    match remote_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}
