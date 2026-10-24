//! Owned asynchronous RPC streams.
//!
//! One physical stream multiplexes replies by XID. Failed or explicitly closed
//! generations are terminal; dropping the final owner aborts the weak reader.
use crate::admission::{Admission, AdmissionPermit};
use crate::{AdmissionError, AdmissionLimits, AdmissionSnapshot};
mod segmented;
pub use segmented::{CallRefusal, ReceiveOutcome, ReplyReceiver, SegmentedReply, SegmentedRequest};

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
enum ResponseSender {
    Buffered {
        sender: oneshot::Sender<Result<RpcMessage, RuntimeError>>,
        permit: Arc<AdmissionPermit>,
    },
    Segmented(Box<dyn segmented::ReceiveTarget>),
}
impl ResponseSender {
    fn fail(self, error: RuntimeError) {
        match self {
            Self::Buffered { sender, permit } => {
                let _ = sender.send(Err(error));
                drop(permit);
            }
            Self::Segmented(target) => target.finish(Err(error)),
        }
    }
}

struct Pending {
    token: Arc<()>,
    sender: Option<ResponseSender>,
}

#[derive(Default)]
struct Replies {
    closed: Option<RuntimeError>,
    pending: HashMap<Xid, Pending>,
}

struct State {
    admission: Admission,
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
        self.admission.close();
        for (_, pending) in pending {
            if let Some(sender) = pending.sender {
                sender.fail(error.clone());
            }
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
    retain_reply: bool,
    state: Arc<State>,
    xid: Xid,
    token: Arc<()>,
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        if self.retain_reply {
            return;
        }
        let removed = {
            let mut replies = self.state.replies();
            if replies.pending.get(&self.xid).is_some_and(|pending| {
                Arc::ptr_eq(&pending.token, &self.token) && pending.sender.is_some()
            }) {
                replies.pending.remove(&self.xid)
            } else {
                None
            }
        };
        // Sender destruction may invoke a custom waker or destroy an application
        // owner. Neither is allowed to run while the reply registry is locked.
        drop(removed);
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
    /// Connect using compatibility-mode admission (no configured call/byte limit).
    pub async fn connect(config: &ClientConfig) -> Result<Self, RuntimeError> {
        Self::from_stream(Self::connect_stream(config).await?, config)
    }

    /// Connect with explicit per-connection admission limits.
    /// Existing client APIs share these limits with owned-receiver calls.
    pub async fn connect_with_admission(
        config: &ClientConfig,
        limits: AdmissionLimits,
    ) -> Result<Self, RuntimeError> {
        let admission =
            Admission::new(limits).map_err(|e| RuntimeError::Transport(e.to_string()))?;
        Self::from_stream_using_admission(
            Self::connect_stream(config).await?,
            config,
            RecordLimits::default(),
            admission,
        )
    }

    async fn connect_stream(config: &ClientConfig) -> Result<tokio::net::TcpStream, RuntimeError> {
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
        Ok(stream)
    }

    /// Observe accepted calls and charged request bytes on this connection.
    pub fn admission_snapshot(&self) -> AdmissionSnapshot {
        self.state.admission.snapshot()
    }

    /// Wait for capacity without transferring a request or reserving it.
    /// Another caller may win admission first; retry `try_call_into` or `try_call`.
    pub async fn wait_for_capacity(&self, required_bytes: usize) -> Result<(), AdmissionError> {
        self.state.admission.wait_for_capacity(required_bytes).await
    }

    pub(crate) fn admission_error(&self, reason: AdmissionError) -> RuntimeError {
        match reason {
            AdmissionError::InvalidRequest(error) => error,
            AdmissionError::Closed => self
                .state
                .replies()
                .closed
                .clone()
                .unwrap_or(RuntimeError::ConnectionClosed),
            other => RuntimeError::Transport(other.to_string()),
        }
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
        Self::from_stream_using_admission(stream, config, limits, Admission::unlimited())
    }

    /// Own a stream with explicit incoming-record and pre-send admission limits.
    /// No payload queue is created for refused calls. Existing constructors keep
    /// compatibility-mode admission; use this constructor to enable bounds.
    pub fn from_stream_with_admission<S>(
        stream: S,
        config: &ClientConfig,
        records: RecordLimits,
        admission: AdmissionLimits,
    ) -> Result<Self, RuntimeError>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let admission =
            Admission::new(admission).map_err(|e| RuntimeError::Transport(e.to_string()))?;
        Self::from_stream_using_admission(stream, config, records, admission)
    }

    fn from_stream_using_admission<S>(
        stream: S,
        config: &ClientConfig,
        limits: RecordLimits,
        admission: Admission,
    ) -> Result<Self, RuntimeError>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        limits.validate()?;
        let (reader, writer) = tokio::io::split(stream);
        let state = Arc::new(State {
            admission,
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
        match self.try_call(request, options) {
            Ok(call) => call.await,
            Err(refusal) => Err(self.admission_error(refusal.reason)),
        }
    }
}

impl OwnedRpcTransport {
    /// Try to admit a classic owned-message call before registration or writing.
    /// Refusal returns the original message and a typed reason; nothing was sent.
    /// Dropping the admitted, unpolled future releases its capacity. Deadlines
    /// start when it is polled. Existing `call` methods expose refusal as a
    /// `RuntimeError` without extending that enum or changing generated signatures.
    #[expect(
        clippy::result_large_err,
        reason = "Returning original owners inline avoids allocating merely to reject a call"
    )]
    pub fn try_call(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<
        impl std::future::Future<Output = Result<RpcMessage, RuntimeError>> + Send + 'static + use<>,
        CallRefusal<RpcMessage>,
    > {
        let bytes = match message_admission_bytes(&request) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Err(CallRefusal {
                    reason: AdmissionError::InvalidRequest(error),
                    request,
                    receiver: (),
                    required_bytes: 0,
                });
            }
        };
        let permit = match self.state.admission.try_acquire(bytes) {
            Ok(permit) => permit,
            Err(reason) => {
                return Err(CallRefusal {
                    reason,
                    request,
                    receiver: (),
                    required_bytes: bytes,
                });
            }
        };
        let permit = Arc::new(permit);
        let completion_permit = Arc::clone(&permit);
        let transport = self.clone();
        let options = options.clone();
        Ok(async move {
            let call = transport.call_inner_admitted(request, permit);
            let result = match options.effective_timeout(transport.default_call_timeout) {
                Some(duration) => timeout(duration, call).await.map_err(|_| {
                    RuntimeError::Transport(format!("call timeout after {duration:?}"))
                })?,
                None => call.await,
            };
            drop(completion_permit);
            result
        })
    }

    async fn call_inner_admitted(
        &self,
        request: RpcMessage,
        permit: Arc<AdmissionPermit>,
    ) -> Result<RpcMessage, RuntimeError> {
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
                    sender: Some(ResponseSender::Buffered { sender, permit }),
                },
            );
        }
        let _pending = PendingCall {
            retain_reply: false,
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
        // Release input owners and the writer before awaiting the reply.
        drop(request);
        receiver.await.map_err(|_| RuntimeError::ConnectionClosed)?
    }
}

// Calculate charge using only a bounded envelope encode. Never coalesce the
// request payload merely to decide whether it can be admitted.
fn message_admission_bytes(message: &RpcMessage) -> Result<usize, RuntimeError> {
    let mut header = message.clone();
    let payload = match &mut header.body {
        MessageBody::Call(call) => std::mem::take(&mut call.payload).len(),
        MessageBody::Reply(crate::ReplyBody::Accepted(crate::AcceptedReply {
            status: crate::AcceptedStatus::Success(bytes),
            ..
        })) => std::mem::take(bytes).len(),
        _ => 0,
    };
    header
        .encode()
        .map_err(RuntimeError::Wire)?
        .len()
        .checked_add(payload)
        .and_then(|n| n.checked_add(4 + std::mem::size_of::<RpcMessage>()))
        .ok_or_else(|| RuntimeError::Transport("RPC admission charge overflow".into()))
}

// A receiver callback or backend panic must not leave a healthy-looking
// generation whose reader task has disappeared. This weak guard cannot keep an
// otherwise unused connection alive.
struct ReaderExit(Weak<State>);
impl Drop for ReaderExit {
    fn drop(&mut self) {
        if let Some(state) = self.0.upgrade() {
            state.fail(RuntimeError::ConnectionClosed);
            if let Ok(mut writer) = state.writer.try_lock() {
                writer.take();
            }
        }
    }
}

async fn reader_loop<R: AsyncRead + Unpin>(
    mut reader: R,
    owner: Weak<State>,
    limits: RecordLimits,
) {
    let _exit = ReaderExit(owner.clone());
    loop {
        let result = segmented::receive_record(&mut reader, &owner, limits).await;
        if let Err(error) = result {
            if let Some(state) = owner.upgrade() {
                state.fail(error);
                state.writer.lock().await.take();
            }
            return;
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
