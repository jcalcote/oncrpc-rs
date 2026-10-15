//! Owned asynchronous RPC streams.
//!
//! One physical stream multiplexes replies by XID. Failed or explicitly closed
//! generations are terminal; dropping the final owner aborts the weak reader.
#[cfg(test)]
use crate::{AcceptedReply, AcceptedStatus, OpaqueAuth, Procedure, ProgramVersion, ReplyBody};
use crate::{
    AsyncClientTransport, CallOptions, ClientConfig, MessageBody, RpcMessage, RuntimeError, Xid,
    try_decode_message_from_buffer, write_rpc_message,
};
#[cfg(test)]
use bytes::Bytes;
use bytes::BytesMut;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    net::TcpSocket,
    sync::{Mutex, MutexGuard, oneshot},
    task::AbortHandle,
    time::timeout,
};

// Bound the full record, including fragment markers, before payload allocation.
const MAX_RECORD_BYTES: usize = 128 * 1024 * 1024;
const MAX_FRAGMENTS: usize = 1024;

/// Bounds on one assembled RPC record, including fragment markers.
#[derive(Clone, Copy, Debug)]
pub struct RecordLimits {
    /// Maximum bytes buffered for one complete record.
    pub max_record_bytes: usize,
    /// Maximum fragments accepted in one record.
    pub max_fragments: usize,
}
impl Default for RecordLimits {
    fn default() -> Self {
        Self {
            max_record_bytes: MAX_RECORD_BYTES,
            max_fragments: MAX_FRAGMENTS,
        }
    }
}
impl RecordLimits {
    fn validate(self) -> Result<(), RuntimeError> {
        if self.max_record_bytes < 4 || self.max_fragments == 0 {
            return Err(RuntimeError::Transport("invalid RPC record limits".into()));
        }
        Ok(())
    }
}

/// A clonable physical RPC connection with explicit, shared terminal state.
#[derive(Clone)]
pub struct OwnedRpcTransport {
    state: Arc<State>,
    default_call_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
}

type Writer = Box<dyn AsyncWrite + Send + Unpin>;
type ResponseSender = oneshot::Sender<Result<RpcMessage, RuntimeError>>;

struct Pending {
    token: Arc<()>,
    sender: ResponseSender,
}

#[derive(Default)]
struct Replies {
    closed: Option<RuntimeError>,
    pending: HashMap<Xid, Pending>,
}

struct State {
    writer: Mutex<Option<Writer>>,
    replies: StdMutex<Replies>,
    reader: OnceLock<AbortHandle>,
    closed: tokio::sync::Notify,
}

impl State {
    fn replies(&self) -> std::sync::MutexGuard<'_, Replies> {
        self.replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn fail(&self, error: RuntimeError) {
        let pending = {
            let mut replies = self.replies();
            if replies.closed.is_some() {
                return;
            }
            replies.closed = Some(error.clone());
            std::mem::take(&mut replies.pending)
        };
        for (_, pending) in pending {
            let _ = pending.sender.send(Err(error.clone()));
        }
        self.closed.notify_waiters();
    }

    fn abort_reader(&self) {
        if let Some(reader) = self.reader.get() {
            reader.abort();
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.abort_reader();
        // The writer drops with this State. The reader holds only a Weak, so
        // it cannot keep an otherwise unused connection or source port alive.
    }
}

struct PendingCall {
    state: Arc<State>,
    xid: Xid,
    token: Arc<()>,
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        let mut replies = self.state.replies();
        if replies
            .pending
            .get(&self.xid)
            .is_some_and(|pending| Arc::ptr_eq(&pending.token, &self.token))
        {
            replies.pending.remove(&self.xid);
        }
    }
}

// If writing is interrupted, another RPC must never follow a partial frame.
// Cancellation while waiting for the writer does not construct this guard.
struct FrameWrite<'a> {
    state: &'a State,
    writer: MutexGuard<'a, Option<Writer>>,
    complete: bool,
}

impl Drop for FrameWrite<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.state.fail(RuntimeError::Transport(
                "RPC frame write interrupted".into(),
            ));
        }
        if self.state.replies().closed.is_some() {
            self.writer.take();
            self.state.abort_reader();
        }
    }
}

impl OwnedRpcTransport {
    /// Connect a plaintext RPC socket and start its owned multiplexed reader.
    pub async fn connect(config: &ClientConfig) -> Result<Self, RuntimeError> {
        let socket = if config.remote_addr.is_ipv4() {
            TcpSocket::new_v4()
        } else {
            TcpSocket::new_v6()
        }
        .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        if let Some(local) = config.local_addr {
            socket
                .bind(local)
                .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        }
        let stream = timeout(config.connect_timeout, socket.connect(config.remote_addr))
            .await
            .map_err(|_| {
                RuntimeError::Transport(format!(
                    "connect timeout after {:?}",
                    config.connect_timeout
                ))
            })?
            .map_err(|error| RuntimeError::Transport(error.to_string()))?;
        Self::from_stream(stream, config)
    }

    /// Mark this generation terminal, fail pending calls, and interrupt its IO.
    /// Pool retirement should instead drop cache ownership to let leased calls finish.
    pub fn close(&self) {
        self.state.fail(RuntimeError::ConnectionClosed);
        self.state.abort_reader();
        if let Ok(mut writer) = self.state.writer.try_lock() {
            writer.take();
        }
    }

    /// Whether this physical connection has failed or been explicitly closed.
    pub fn is_closed(&self) -> bool {
        self.state.replies().closed.is_some()
    }

    /// Wait until this physical connection becomes terminal.
    pub async fn wait_closed(&self) {
        loop {
            let notification = self.state.closed.notified();
            tokio::pin!(notification);
            notification.as_mut().enable();
            if self.is_closed() {
                return;
            }
            notification.await;
        }
    }
    /// Own a byte stream using the default 128 MiB / 1,024-fragment record limits.
    pub fn from_stream<S>(stream: S, config: &ClientConfig) -> Result<Self, RuntimeError>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        Self::from_stream_with_limits(stream, config, RecordLimits::default())
    }

    /// Own a byte stream with explicit record assembly limits.
    pub fn from_stream_with_limits<S>(
        stream: S,
        config: &ClientConfig,
        limits: RecordLimits,
    ) -> Result<Self, RuntimeError>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        limits.validate()?;
        let (reader, writer) = tokio::io::split(stream);
        let state = Arc::new(State {
            writer: Mutex::new(Some(Box::new(writer))),
            replies: StdMutex::new(Replies::default()),
            reader: OnceLock::new(),
            closed: tokio::sync::Notify::new(),
        });
        let task = tokio::spawn(reader_loop(reader, Arc::downgrade(&state), limits));
        let _ = state.reader.set(task.abort_handle());
        Ok(Self {
            state,
            default_call_timeout: config.default_call_timeout,
            write_timeout: config.write_timeout,
        })
    }

    async fn write(&self, message: &RpcMessage) -> Result<(), RuntimeError> {
        tokio::select! {
            biased;
            () = self.wait_closed() => Err(self.state.replies().closed.clone().unwrap_or(RuntimeError::ConnectionClosed)),
            result = self.write_frame(message) => result,
        }
    }

    async fn write_frame(&self, message: &RpcMessage) -> Result<(), RuntimeError> {
        let writer = self.state.writer.lock().await;
        if let Some(error) = self.state.replies().closed.clone() {
            return Err(error);
        }
        let mut frame = FrameWrite {
            state: &self.state,
            writer,
            complete: false,
        };
        let writer = frame
            .writer
            .as_mut()
            .ok_or(RuntimeError::ConnectionClosed)?;
        let result = write_rpc_message(writer, message).await;
        match &result {
            Ok(()) => frame.complete = true,
            Err(error) => self.state.fail(error.clone()),
        }
        result
    }
}

#[async_trait::async_trait]
impl AsyncClientTransport for OwnedRpcTransport {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        self.call_with_options(request, &CallOptions::default())
            .await
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        match options.effective_timeout(self.default_call_timeout) {
            Some(duration) => timeout(duration, self.call_inner(request))
                .await
                .map_err(|_| RuntimeError::Transport(format!("call timeout after {duration:?}")))?,
            None => self.call_inner(request).await,
        }
    }
}

impl OwnedRpcTransport {
    async fn call_inner(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        let (sender, receiver) = oneshot::channel();
        let token = Arc::new(());
        {
            let mut replies = self.state.replies();
            if let Some(error) = &replies.closed {
                return Err(error.clone());
            }
            if replies.pending.contains_key(&request.xid) {
                return Err(RuntimeError::Transport(
                    "duplicate in-flight RPC XID".into(),
                ));
            }
            replies.pending.insert(
                request.xid,
                Pending {
                    token: token.clone(),
                    sender,
                },
            );
        }
        let _pending = PendingCall {
            state: self.state.clone(),
            xid: request.xid,
            token,
        };
        match self.write_timeout {
            Some(duration) => timeout(duration, self.write(&request))
                .await
                .map_err(|_| {
                    RuntimeError::Transport(format!("write timeout after {duration:?}"))
                })??,
            None => self.write(&request).await?,
        }
        // Release the writer before awaiting the reply: requests are multiplexed.
        receiver.await.map_err(|_| RuntimeError::ConnectionClosed)?
    }
}

async fn reader_loop<R: AsyncRead + Unpin>(
    mut reader: R,
    owner: Weak<State>,
    limits: RecordLimits,
) {
    loop {
        let message = read_rpc_message_with_limits(&mut reader, limits)
            .await
            .and_then(|message| {
                if matches!(message.body, MessageBody::Reply(_)) {
                    Ok(message)
                } else {
                    Err(RuntimeError::UnexpectedCallMessage)
                }
            });
        let Some(state) = owner.upgrade() else {
            return;
        };
        match message {
            Ok(message) => {
                let pending = state.replies().pending.remove(&message.xid);
                if let Some(pending) = pending {
                    let _ = pending.sender.send(Ok(message));
                }
            }
            Err(error) => {
                state.fail(error);
                state.writer.lock().await.take();
                return;
            }
        }
    }
}

/// Read one bounded, possibly fragmented RPC record without consuming the next.
pub async fn read_rpc_message<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<RpcMessage, RuntimeError> {
    read_rpc_message_with_limits(reader, RecordLimits::default()).await
}

/// Read one RPC record with explicit bounds before allocating fragment payloads.
pub async fn read_rpc_message_with_limits<R: AsyncRead + Unpin>(
    reader: &mut R,
    limits: RecordLimits,
) -> Result<RpcMessage, RuntimeError> {
    limits.validate()?;
    let mut record = BytesMut::new();
    for _ in 0..limits.max_fragments {
        let mut header = [0; 4];
        reader.read_exact(&mut header).await.map_err(read_error)?;
        let marker = u32::from_be_bytes(header);
        let length = usize::try_from(marker & 0x7fff_ffff)
            .map_err(|_| RuntimeError::Transport("RPC fragment length overflow".into()))?;
        let end = record
            .len()
            .checked_add(4)
            .and_then(|len| len.checked_add(length))
            .filter(|end| *end <= limits.max_record_bytes)
            .ok_or_else(|| RuntimeError::Transport("RPC record exceeds configured limit".into()))?;
        record.extend_from_slice(&header);
        let start = record.len();
        record.resize(end, 0);
        reader
            .read_exact(&mut record[start..])
            .await
            .map_err(read_error)?;
        if marker & 0x8000_0000 != 0 {
            return try_decode_message_from_buffer(&mut record)?
                .ok_or(RuntimeError::ConnectionClosed);
        }
    }
    Err(RuntimeError::Transport(
        "RPC record has too many fragments".into(),
    ))
}

fn read_error(error: io::Error) -> RuntimeError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        RuntimeError::ConnectionClosed
    } else {
        RuntimeError::Transport(error.to_string())
    }
}

#[cfg(test)]
mod tests;
