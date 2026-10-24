use crate::{
    AsyncClientTransport, CallOptions, ClientConfig, ClientTransport, OwnedRpcTransport,
    RpcMessage, RuntimeError, Xid, XidAllocator, decode_rpc_message_datagram,
    encode_rpc_message_datagram,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::time::timeout;

type PendingMap = Arc<StdMutex<HashMap<Xid, UdpReply>>>;

struct UdpReply {
    token: Arc<()>,
    sender: oneshot::Sender<Result<RpcMessage, RuntimeError>>,
}

/// Remove this registration even when its future is dropped during socket I/O.
struct UdpPendingCall {
    pending: PendingMap,
    xid: Xid,
    token: Arc<()>,
}
impl Drop for UdpPendingCall {
    fn drop(&mut self) {
        let removed = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending
                .get(&self.xid)
                .is_some_and(|entry| Arc::ptr_eq(&entry.token, &self.token))
            {
                pending.remove(&self.xid)
            } else {
                None
            }
        };
        // A sender's waker must not run while the registry is locked.
        drop(removed);
    }
}

#[derive(Clone)]
pub struct TokioAsyncClientTransport {
    inner: OwnedRpcTransport,
}

#[derive(Clone)]
pub struct TokioAsyncUdpClientTransport {
    socket: Arc<UdpSocket>,
    pending: PendingMap,
    default_call_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
    xids: XidAllocator,
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
        Ok(Self {
            inner: OwnedRpcTransport::connect(config).await?,
        })
    }
    pub fn close(&self) {
        self.inner.close();
    }
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
    pub async fn wait_closed(&self) {
        self.inner.wait_closed().await;
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
        let pending = Arc::new(StdMutex::new(HashMap::new()));

        tokio::spawn(udp_reader_task(socket.clone(), pending.clone()));

        Ok(Self {
            socket,
            pending,
            default_call_timeout,
            write_timeout,
            xids: XidAllocator::new(),
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
    fn xid_allocator(&self) -> Option<XidAllocator> {
        Some(self.inner.xid_allocator())
    }
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

#[async_trait]
impl AsyncClientTransport for TokioAsyncUdpClientTransport {
    fn xid_allocator(&self) -> Option<XidAllocator> {
        Some(self.xids.clone())
    }
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
            .await
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        if options
            .cancellation
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
        {
            return Err(RuntimeError::Cancelled);
        }
        let xid = request.xid;
        let payload = encode_rpc_message_datagram(&request)?;
        let (tx, rx) = oneshot::channel();
        let token = Arc::new(());
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.contains_key(&xid) {
                return Err(RuntimeError::Transport(
                    "duplicate in-flight RPC XID".into(),
                ));
            }
            pending.insert(
                xid,
                UdpReply {
                    token: token.clone(),
                    sender: tx,
                },
            );
        }
        let _registration = UdpPendingCall {
            pending: self.pending.clone(),
            xid,
            token,
        };
        let operation = async {
            send_udp_request(&self.socket, &payload, self.write_timeout).await?;
            rx.await.map_err(|_| RuntimeError::ConnectionClosed)?
        };
        let deadline = async {
            match options.effective_timeout(self.default_call_timeout) {
                Some(duration) => timeout(duration, operation)
                    .await
                    .map_err(|_| RuntimeError::CallTimeout { duration })?,
                None => operation.await,
            }
        };
        let cancellation = async {
            match &options.cancellation {
                Some(cancel) => cancel.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        // Cancellation detaches the local waiter. The datagram may already have
        // reached the server; the remote operation is never undone or resent.
        tokio::select! {
            biased;
            () = cancellation => Err(RuntimeError::Cancelled),
            result = deadline => result,
        }
    }
}

impl ClientTransport for TokioClientTransport {
    fn xid_allocator(&self) -> Option<XidAllocator> {
        self.inner.xid_allocator()
    }
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
    fn xid_allocator(&self) -> Option<XidAllocator> {
        self.inner.xid_allocator()
    }
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

async fn send_udp_request(
    socket: &Arc<UdpSocket>,
    payload: &bytes::Bytes,
    write_timeout: Option<std::time::Duration>,
) -> Result<(), RuntimeError> {
    let write_future = socket.send(payload);
    let write_result = match write_timeout {
        Some(duration) => timeout(duration, write_future)
            .await
            .map_err(|_| RuntimeError::WriteTimeout { duration })?,
        None => write_future.await,
    };

    write_result
        .map(|_| ())
        .map_err(|error| RuntimeError::Transport(error.to_string()))
}

async fn udp_reader_task(socket: Arc<UdpSocket>, pending: PendingMap) {
    let result = udp_reader_loop(socket, pending.clone()).await;
    if let Err(error) = result {
        fail_pending(&pending, error).await;
    }
}

async fn fail_pending(pending: &PendingMap, error: RuntimeError) {
    let entries = {
        let mut pending = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *pending)
    };
    for (_, entry) in entries {
        let _ = entry.sender.send(Err(error.clone()));
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
        let entry = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&xid);
        if let Some(entry) = entry {
            let _ = entry.sender.send(Ok(message));
        }
    }
}

fn udp_unspecified_addr(remote_addr: SocketAddr) -> SocketAddr {
    match remote_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

#[cfg(test)]
mod udp_cancellation_tests {
    use super::*;
    use crate::{CallCancellation, MessageBody, OpaqueAuth, Procedure, ProgramVersion};
    use bytes::Bytes;
    use std::time::Duration;

    fn request(xid: u32) -> RpcMessage {
        RpcMessage {
            xid: Xid(xid),
            body: MessageBody::Call(onc_rpc_wire::CallBody::new(
                ProgramVersion {
                    program: 42,
                    version: 1,
                },
                Procedure(1),
                OpaqueAuth::none(),
                OpaqueAuth::none(),
                Bytes::new(),
            )),
        }
    }

    #[tokio::test]
    async fn cancellation_timeout_and_future_drop_remove_udp_waiters() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = ClientConfig::new(socket.local_addr().unwrap());
        let transport = TokioAsyncUdpClientTransport::connect(&config)
            .await
            .unwrap();
        for mode in 0..3 {
            let cancellation = CallCancellation::new();
            let duration = Duration::from_secs(30);
            let options = CallOptions::new()
                .with_cancellation(cancellation.clone())
                .with_timeout(duration);
            let mut call = Box::pin(transport.call_with_options(request(mode + 1), &options));
            let mut bytes = [0; 512];
            // Actual receipt establishes that the registration and send finished.
            tokio::select! {
                result = &mut call => panic!("call completed before server reply: {result:?}"),
                received = socket.recv_from(&mut bytes) => { received.unwrap(); }
            }
            assert_eq!(transport.pending.lock().unwrap().len(), 1);
            match mode {
                0 => {
                    cancellation.cancel();
                    assert!(matches!(call.await, Err(RuntimeError::Cancelled)));
                }
                1 => drop(call),
                2 => {
                    // Start virtual time only after socket readiness was observed.
                    tokio::time::pause();
                    tokio::time::advance(duration).await;
                    assert!(
                        matches!(call.await, Err(RuntimeError::CallTimeout { duration: actual }) if actual == duration)
                    );
                    tokio::time::resume();
                }
                _ => unreachable!(),
            }
            assert!(transport.pending.lock().unwrap().is_empty());
        }
    }
}
