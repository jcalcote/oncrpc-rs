//! Owned asynchronous RPC streams.
//!
//! One physical stream multiplexes replies by XID. Failed or explicitly closed
//! generations are terminal; dropping the final owner aborts the weak reader.
use crate::admission::{Admission, AdmissionPermit};
use crate::{AdmissionError, AdmissionLimits, AdmissionSnapshot};
mod segmented;
pub use segmented::{CallRefusal, ReceiveOutcome, ReplyReceiver, SegmentedReply, SegmentedRequest};

#[cfg(test)]
use crate::{
    AcceptedReply, AcceptedStatus, OpaqueAuth, Procedure, ProgramVersion, ReplyBody,
    write_rpc_message,
};
use crate::{
    AsyncClientTransport, CallOptions, ClientConfig, MessageBody, RpcMessage, RuntimeError, Xid,
    XidAllocator, try_decode_message_from_buffer,
};
#[cfg(test)]
use bytes::Bytes;
use bytes::BytesMut;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex as StdMutex, OnceLock, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    net::TcpSocket,
    sync::{Mutex, MutexGuard, oneshot},
    task::AbortHandle,
    time::{Instant, timeout},
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
    token: Arc<CallControl>,
    sender: Option<ResponseSender>,
}

#[derive(Default)]
struct Replies {
    closed: Option<RuntimeError>,
    pending: HashMap<Xid, Pending>,
}

struct State {
    xids: XidAllocator,
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
            pending.token.stop(error.clone());
            if let Some(sender) = pending.sender {
                sender.fail(
                    pending
                        .token
                        .finish(Err::<(), _>(error.clone()))
                        .unwrap_err(),
                );
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

// A single terminal decision is shared by the caller and the stream reader.
// Waking tasks and delivering application owners always happens outside locks.
#[derive(Default)]
struct CallControl {
    terminal: StdMutex<Option<Result<(), RuntimeError>>>,
    changed: tokio::sync::Notify,
}
impl CallControl {
    fn stop(&self, error: RuntimeError) {
        let notify = {
            let mut terminal = self
                .terminal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if terminal.is_some() {
                false
            } else {
                *terminal = Some(Err(error));
                true
            }
        };
        if notify {
            self.changed.notify_waiters();
        }
    }
    fn error(&self) -> Option<RuntimeError> {
        self.terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|result| result.as_ref().err().cloned())
    }
    fn finish<T>(&self, result: Result<T, RuntimeError>) -> Result<T, RuntimeError> {
        let mut terminal = self
            .terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match terminal.as_ref() {
            Some(Err(error)) => Err(error.clone()),
            _ => {
                *terminal = Some(Ok(()));
                result
            }
        }
    }
    async fn stopped(&self) -> RuntimeError {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(error) = self.error() {
                return error;
            }
            changed.await;
        }
    }
}

struct PendingCall {
    state: Arc<State>,
    xid: Xid,
    token: Arc<CallControl>,
}
impl PendingCall {
    fn stop(&self, error: RuntimeError) {
        self.token.stop(error.clone());
        let removed = {
            let mut replies = self.state.replies();
            if replies
                .pending
                .get(&self.xid)
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &self.token))
            {
                replies.pending.remove(&self.xid)
            } else {
                None
            }
        };
        if let Some(sender) = removed.and_then(|pending| pending.sender) {
            sender.fail(self.token.finish(Err::<(), _>(error)).unwrap_err());
        }
    }
}
impl Drop for PendingCall {
    fn drop(&mut self) {
        self.stop(RuntimeError::Cancelled);
    }
}

// Track actual stream writes: dropping before the first byte is harmless;
// dropping after a partial frame cannot leave that stream reusable.
struct FrameWrite<'a> {
    state: &'a State,
    writer: MutexGuard<'a, Option<Writer>>,
    written: &'a AtomicUsize,
    complete: bool,
}
impl AsyncWrite for FrameWrite<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(self.writer.as_mut().expect("live writer")).poll_write(cx, bytes);
        if let Poll::Ready(Ok(count)) = result {
            self.written.fetch_add(count, Ordering::Relaxed);
        }
        result
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result =
            Pin::new(self.writer.as_mut().expect("live writer")).poll_write_vectored(cx, bytes);
        if let Poll::Ready(Ok(count)) = result {
            self.written.fetch_add(count, Ordering::Relaxed);
        }
        result
    }
    fn is_write_vectored(&self) -> bool {
        self.writer
            .as_ref()
            .expect("live writer")
            .is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.writer.as_mut().expect("live writer")).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.writer.as_mut().expect("live writer")).poll_shutdown(cx)
    }
}
impl Drop for FrameWrite<'_> {
    fn drop(&mut self) {
        if !self.complete && self.written.load(Ordering::Relaxed) != 0 {
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

struct CallWait<'a> {
    options: &'a CallOptions,
    deadline: Option<(Instant, Duration)>,
}
impl<'a> CallWait<'a> {
    fn new(options: &'a CallOptions, default: Option<Duration>) -> Self {
        Self {
            options,
            deadline: options.effective_timeout(default).and_then(|duration| {
                Instant::now()
                    .checked_add(duration)
                    .map(|at| (at, duration))
            }),
        }
    }
    async fn interrupted(&self) -> RuntimeError {
        let cancel = async {
            match &self.options.cancellation {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            () = cancel => RuntimeError::Cancelled,
            () = wait_deadline(self.deadline.map(|(at, _)| at)) => RuntimeError::CallTimeout { duration: self.deadline.expect("expired deadline").1 },
        }
    }
}
async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

impl OwnedRpcTransport {
    /// Shared XID namespace for raw calls and every client using this stream.
    pub fn xid_allocator(&self) -> XidAllocator {
        self.state.xids.clone()
    }

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
            xids: XidAllocator::new(),
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

    async fn send_segments(
        &self,
        segments: &[bytes::Bytes],
        pending: &PendingCall,
        wait: &CallWait<'_>,
    ) -> Result<(), RuntimeError> {
        let write_deadline = self
            .write_timeout
            .and_then(|duration| Instant::now().checked_add(duration));
        let writer = tokio::select! {
            biased;
            error = wait.interrupted() => return Err(error),
            () = self.wait_closed() => return Err(self.state.replies().closed.clone().unwrap_or(RuntimeError::ConnectionClosed)),
            () = wait_deadline(write_deadline) => return Err(RuntimeError::WriteTimeout { duration: self.write_timeout.expect("write deadline") }),
            writer = self.state.writer.lock() => writer,
        };
        if let Some(error) = self.state.replies().closed.clone() {
            return Err(error);
        }
        if writer.is_none() {
            return Err(RuntimeError::ConnectionClosed);
        }
        let written = AtomicUsize::new(0);
        let mut frame = FrameWrite {
            state: &self.state,
            writer,
            written: &written,
            complete: false,
        };
        let result = {
            let write = async {
                crate::record_io::write_segments(&mut frame, segments).await?;
                tokio::io::AsyncWriteExt::flush(&mut frame)
                    .await
                    .map_err(|error| RuntimeError::Transport(error.to_string()))
            };
            tokio::pin!(write);
            // A soft stop does not interrupt a frame. The physical write remains
            // governed by its separate timeout and explicit connection closure.
            let mut stopped = false;
            loop {
                tokio::select! {
                    biased;
                    () = self.wait_closed() => break Err(self.state.replies().closed.clone().unwrap_or(RuntimeError::ConnectionClosed)),
                    () = wait_deadline(write_deadline) => break Err(RuntimeError::WriteTimeout { duration: self.write_timeout.expect("write deadline") }),
                    error = wait.interrupted(), if !stopped => {
                        pending.stop(error.clone());
                        if written.load(Ordering::Relaxed) == 0 { break Err(error); }
                        stopped = true;
                    }
                    result = &mut write => break result,
                }
            }
        };
        match &result {
            Ok(()) => frame.complete = true,
            Err(error)
                if written.load(Ordering::Relaxed) == 0
                    && matches!(
                        error,
                        RuntimeError::Cancelled
                            | RuntimeError::CallTimeout { .. }
                            | RuntimeError::WriteTimeout { .. }
                    ) => {}
            Err(error) => self.state.fail(error.clone()),
        }
        result
    }

    async fn complete_call<T>(
        &self,
        segments: Vec<bytes::Bytes>,
        pending: &PendingCall,
        wait: &CallWait<'_>,
        mut response: oneshot::Receiver<T>,
    ) -> Result<T, RuntimeError> {
        if let Err(error) = self.send_segments(&segments, pending, wait).await {
            pending.stop(error);
        }
        drop(segments);
        tokio::select! {
            biased;
            result = &mut response => result.map_err(|_| pending.token.error().unwrap_or(RuntimeError::ConnectionClosed)),
            error = wait.interrupted() => {
                pending.stop(error);
                response.await.map_err(|_| pending.token.error().unwrap_or(RuntimeError::ConnectionClosed))
            }
        }
    }
}

#[async_trait::async_trait]
impl AsyncClientTransport for OwnedRpcTransport {
    fn xid_allocator(&self) -> Option<XidAllocator> {
        Some(OwnedRpcTransport::xid_allocator(self))
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
    /// `RuntimeError` without changing generated signatures.
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
            let result = transport
                .call_inner_admitted(request, &options, permit)
                .await;
            drop(completion_permit);
            result
        })
    }

    async fn call_inner_admitted(
        &self,
        request: RpcMessage,
        options: &CallOptions,
        permit: Arc<AdmissionPermit>,
    ) -> Result<RpcMessage, RuntimeError> {
        // The compatibility path already assembles its request. Owned segments
        // use the same lifecycle without that assembly.
        let segments = vec![request.encode().map_err(RuntimeError::Wire)?];
        let (sender, receiver) = oneshot::channel();
        let token = Arc::new(CallControl::default());
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
        let pending = PendingCall {
            state: self.state.clone(),
            xid: request.xid,
            token,
        };
        drop(request);
        self.complete_call(
            segments,
            &pending,
            &CallWait::new(options, self.default_call_timeout),
            receiver,
        )
        .await?
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
