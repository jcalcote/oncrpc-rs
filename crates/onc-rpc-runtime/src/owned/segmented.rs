//! Optional caller-owned buffer path; legacy transports keep their API.
use super::*;
use crate::record_io::{RecordReader, write_segments};
use bytes::{Bytes, BytesMut};

/// A small RPC call envelope followed by immutable owned payload segments.
///
/// `header` must be a call with an empty payload. The supplied segments contain
/// the complete XDR arguments, including any application header and padding.
/// `Bytes::from_owner` can retain custom allocations without copying them.
#[derive(Debug, Clone)]
pub struct SegmentedRequest {
    /// Call envelope with an empty payload; XID must be fresh on this connection.
    pub header: RpcMessage,
    /// Complete XDR arguments, including application headers and padding.
    pub payload: Vec<Bytes>,
}

impl SegmentedRequest {
    fn prepare(&self) -> Result<(Bytes, usize), RuntimeError> {
        match &self.header.body {
            MessageBody::Call(call) if call.payload.is_empty() => {}
            _ => {
                return Err(RuntimeError::Transport(
                    "segmented request requires an empty call envelope".into(),
                ));
            }
        }
        let header = self.header.encode().map_err(RuntimeError::Wire)?;
        let wire_bytes = self
            .payload
            .iter()
            .try_fold(header.len(), |n, segment| n.checked_add(segment.len()))
            .filter(|n| *n <= crate::MAX_FRAGMENT_LEN as usize)
            .ok_or_else(|| {
                RuntimeError::Transport("segmented request exceeds RPC fragment limit".into())
            })?;
        let descriptor_bytes = self
            .payload
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_mul(std::mem::size_of::<Bytes>()))
            .ok_or_else(|| RuntimeError::Transport("RPC descriptor charge overflow".into()))?;
        let charge = wire_bytes
            .checked_add(4)
            .and_then(|n| n.checked_add(descriptor_bytes))
            .ok_or_else(|| RuntimeError::Transport("RPC admission charge overflow".into()))?;
        Ok((header, charge))
    }
}

/// A pre-send refusal containing both original input owners.
/// Nothing was registered or written. Retry only transient `AtCapacity` errors;
/// retain these owners in an application-controlled budget while waiting.
pub struct CallRefusal<Q, R = ()> {
    /// Typed refusal, distinct from network or reply timeouts.
    pub reason: AdmissionError,
    /// Original request, including immutable payload owners.
    pub request: Q,
    /// Original receiver; unit for classic calls.
    pub receiver: R,
    /// Logical request/descriptor/inline-owner charge used for admission.
    pub required_bytes: usize,
}
impl<Q, R> std::fmt::Debug for CallRefusal<Q, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallRefusal")
            .field("reason", &self.reason)
            .field("required_bytes", &self.required_bytes)
            .finish_non_exhaustive()
    }
}
impl<Q, R> std::fmt::Display for CallRefusal<Q, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(f)
    }
}
impl<Q, R> std::error::Error for CallRefusal<Q, R> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.reason)
    }
}

/// A decoded envelope and caller-owned buffers filled with its XDR results.
///
/// Only the first `payload_len` bytes across `buffers` were received. Successful
/// RPC envelopes carry an empty `Success` payload: result bytes live exclusively
/// in these buffers. A remote reject is returned as its original RPC envelope.
#[derive(Debug)]
pub struct SegmentedReply<B> {
    /// RPC envelope; successful result bytes live in `buffers`.
    pub header: RpcMessage,
    /// Original destinations, in their supplied order.
    pub buffers: Vec<B>,
    /// Total result bytes received across those destinations.
    pub payload_len: usize,
}

/// An owned destination that can choose a new writable range after each read.
///
/// All returned bytes must be initialized. An empty slice means that this
/// receiver is full. `advance` is called only for bytes actually read, before
/// requesting the next range. It can parse a small application prefix and then
/// expose a payload range without copying that payload.
pub trait ReplyReceiver: Send + 'static {
    /// Supply the next writable range, or an empty slice when capacity is full.
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError>;
    /// Commit a positive count received into the most recently supplied range.
    fn advance(&mut self, bytes: usize) -> Result<(), RuntimeError>;
}

/// Final, quiescent result of an owned receive, including the original owner.
///
/// The library no longer accesses `receiver`, even when `result` is an error.
/// `payload_len` counts bytes placed in the receiver before completion/failure.
#[derive(Debug)]
pub struct ReceiveOutcome<R> {
    /// Local completion or RPC envelope. Remote accept/reject errors remain in the envelope.
    pub result: Result<RpcMessage, RuntimeError>,
    /// Original receiver, returned only after the last library access.
    pub receiver: R,
    /// XDR result bytes placed before completion, including partial reads on error.
    pub payload_len: usize,
}

pub(super) trait ReceiveTarget: Send {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError>;
    fn advance(&mut self, bytes: usize) -> Result<(), RuntimeError>;
    fn finish(self: Box<Self>, header: Result<RpcMessage, RuntimeError>);
}
struct Target<R> {
    receiver: Option<R>,
    sender: Option<oneshot::Sender<ReceiveOutcome<R>>>,
    payload_len: usize,
    permit: Option<Arc<AdmissionPermit>>,
}
impl<R> Target<R> {
    fn deliver(&mut self, result: Result<RpcMessage, RuntimeError>) {
        if let (Some(receiver), Some(sender)) = (self.receiver.take(), self.sender.take()) {
            let permit = self.permit.take();
            let _ = sender.send(ReceiveOutcome {
                result,
                receiver,
                payload_len: self.payload_len,
            });
            // A failed send destroys an abandoned receiver before releasing its
            // admission charge. A live future also retains its completion lease.
            drop(permit);
        }
    }
}
impl<R> Drop for Target<R> {
    fn drop(&mut self) {
        // Aborting the reader drops its in-flight read borrow before this owner.
        // Return the receiver only at that physical cleanup boundary.
        self.deliver(Err(RuntimeError::ConnectionClosed));
    }
}
impl<R: ReplyReceiver> ReceiveTarget for Target<R> {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        self.receiver.as_mut().expect("live receive owner").buffer()
    }
    fn advance(&mut self, bytes: usize) -> Result<(), RuntimeError> {
        self.payload_len = self
            .payload_len
            .checked_add(bytes)
            .ok_or_else(|| RuntimeError::Transport("RPC receive byte count overflow".into()))?;
        self.receiver
            .as_mut()
            .expect("live receive owner")
            .advance(bytes)
    }
    fn finish(mut self: Box<Self>, header: Result<RpcMessage, RuntimeError>) {
        self.deliver(header);
    }
}

struct Buffers<B> {
    buffers: Vec<B>,
    index: usize,
    offset: usize,
}
impl<B: AsMut<[u8]> + Send + 'static> ReplyReceiver for Buffers<B> {
    fn buffer(&mut self) -> Result<&mut [u8], RuntimeError> {
        while self.index < self.buffers.len()
            && self.offset == self.buffers[self.index].as_mut().len()
        {
            self.index += 1;
            self.offset = 0;
        }
        if self.index == self.buffers.len() {
            return Ok(&mut []);
        }
        Ok(&mut self.buffers[self.index].as_mut()[self.offset..])
    }
    fn advance(&mut self, bytes: usize) -> Result<(), RuntimeError> {
        self.offset += bytes;
        Ok(())
    }
}

impl OwnedRpcTransport {
    /// Send scatter/gather arguments and receive directly into owned buffers.
    ///
    /// Dropping a fully sent call leaves its XID and buffers owned by the
    /// connection until its reply is consumed or the connection is closed.
    /// Cancellation during a frame write retires the connection. Deadlines
    /// return only after the destination is quiescent; use `call_into` to recover
    /// the original destination on errors as well as success. Dropping the
    /// returned future never exposes storage still accessed by the reader.
    /// Ordinary socket copies remain; there is no full RPC payload staging copy.
    ///
    /// ```no_run
    /// # use bytes::Bytes;
    /// # use onc_rpc_runtime::*;
    /// # async fn example(transport: &OwnedRpcTransport) -> Result<(), RuntimeError> {
    /// let request = SegmentedRequest {
    ///     header: RpcMessage {
    ///         xid: Xid(1), // Allocate a fresh XID for every call on this connection.
    ///         body: MessageBody::Call(onc_rpc_wire::CallBody::new(
    ///             ProgramVersion { program: 0x3123_0001, version: 1 },
    ///             Procedure(1), OpaqueAuth::none(), OpaqueAuth::none(), Bytes::new(),
    ///         )),
    ///     },
    ///     // Complete application XDR arguments, split without coalescing.
    ///     payload: vec![Bytes::from_static(b"abcd"), Bytes::from_static(b"efgh")],
    /// };
    /// let reply = transport.call_segmented(
    ///     request, vec![vec![0u8; 4], vec![0u8; 4]], &CallOptions::default(),
    /// ).await?;
    /// // Inspect the RPC status before decoding the XDR results in the buffers.
    /// if let MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
    ///     status: AcceptedStatus::Success(_), ..
    /// })) = reply.header.body {
    ///     assert!(reply.payload_len <= 8);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn call_segmented<B: AsMut<[u8]> + Send + 'static>(
        &self,
        request: SegmentedRequest,
        buffers: Vec<B>,
        options: &CallOptions,
    ) -> Result<SegmentedReply<B>, RuntimeError> {
        let outcome = self
            .call_into(
                request,
                Buffers {
                    buffers,
                    index: 0,
                    offset: 0,
                },
                options,
            )
            .await;
        outcome.result.map(|header| SegmentedReply {
            header,
            buffers: outcome.receiver.buffers,
            payload_len: outcome.payload_len,
        })
    }

    /// Receive an RPC result into an application-directed owned destination.
    ///
    /// This convenience method reports admission refusal as `RuntimeError` and
    /// drops the unsent request. Use `try_call_into` for typed refusal and recovery
    /// of both input owners.
    ///
    /// Every returned outcome contains the original receiver after the final
    /// library access. A deadline after writing starts retires the connection
    /// and waits for that receiver to become quiescent. A deadline while queued
    /// for the writer removes only this unsent call. Dropping this future can
    /// abandon its result; the reader still retains the receiver until cleanup.
    /// To cancel and recover ownership, call `close` and continue awaiting this
    /// future. Receiver callback errors retire the connection.
    pub async fn call_into<R: ReplyReceiver>(
        &self,
        request: SegmentedRequest,
        receiver: R,
        options: &CallOptions,
    ) -> ReceiveOutcome<R> {
        match self.try_call_into(request, receiver, options) {
            Ok(call) => call.await,
            Err(refusal) => ReceiveOutcome {
                result: Err(self.admission_error(refusal.reason)),
                receiver: refusal.receiver,
                payload_len: 0,
            },
        }
    }

    /// Try to reserve this call's capacity before allocating transport payload
    /// descriptors, registering its XID or writing bytes. Admission is immediate;
    /// the returned future performs I/O when polled. A refusal returns both owners.
    /// Dropping an unpolled future returns its permit. Active permits remain held
    /// until the reader releases the destination, including abandoned replies.
    /// Call/write deadlines start when the admitted future is polled.
    /// Refusal never closes the connection; the caller decides whether to wait,
    /// retry or close it.
    ///
    /// Charge covers serialized request bytes, a marker, outgoing `Bytes`
    /// descriptors and the inline receiver size, not receiver-owned heap storage
    /// or the capacity of shared backing allocations. Limits are per connection.
    #[expect(
        clippy::result_large_err,
        reason = "Returning original owners inline avoids allocating merely to reject a call"
    )]
    pub fn try_call_into<R: ReplyReceiver>(
        &self,
        request: SegmentedRequest,
        receiver: R,
        options: &CallOptions,
    ) -> Result<
        impl std::future::Future<Output = ReceiveOutcome<R>> + Send + 'static + use<R>,
        CallRefusal<SegmentedRequest, R>,
    > {
        let (header, charge) = match request.prepare() {
            Ok(value) => value,
            Err(error) => {
                return Err(CallRefusal {
                    reason: AdmissionError::InvalidRequest(error),
                    request,
                    receiver,
                    required_bytes: 0,
                });
            }
        };
        let required_bytes = match charge.checked_add(std::mem::size_of::<R>()) {
            Some(value) => value,
            None => {
                return Err(CallRefusal {
                    reason: AdmissionError::InvalidRequest(RuntimeError::Transport(
                        "RPC receiver charge overflow".into(),
                    )),
                    request,
                    receiver,
                    required_bytes: 0,
                });
            }
        };
        let permit = match self.state.admission.try_acquire(required_bytes) {
            Ok(permit) => permit,
            Err(reason) => {
                return Err(CallRefusal {
                    reason,
                    request,
                    receiver,
                    required_bytes,
                });
            }
        };
        let xid = request.header.xid;
        let mut segments = Vec::with_capacity(request.payload.len() + 1);
        segments.push(header);
        segments.extend(request.payload);
        let permit = Arc::new(permit);
        let completion_permit = Arc::clone(&permit);
        let transport = self.clone();
        let options = options.clone();
        Ok(async move {
            let outcome = transport
                .call_into_admitted(xid, segments, receiver, &options, permit)
                .await;
            drop(completion_permit);
            outcome
        })
    }

    async fn call_into_admitted<R: ReplyReceiver>(
        &self,
        xid: Xid,
        segments: Vec<Bytes>,
        receiver: R,
        options: &CallOptions,
        permit: Arc<AdmissionPermit>,
    ) -> ReceiveOutcome<R> {
        let (sender, mut response) = oneshot::channel();
        let token = Arc::new(());
        {
            let mut replies = self.state.replies();
            let error = replies.closed.clone().or_else(|| {
                replies
                    .pending
                    .contains_key(&xid)
                    .then(|| RuntimeError::Transport("duplicate in-flight RPC XID".into()))
            });
            if let Some(error) = error {
                return ReceiveOutcome {
                    result: Err(error),
                    receiver,
                    payload_len: 0,
                };
            }
            replies.pending.insert(
                xid,
                Pending {
                    token: token.clone(),
                    sender: Some(ResponseSender::Segmented(Box::new(Target {
                        receiver: Some(receiver),
                        sender: Some(sender),
                        payload_len: 0,
                        permit: Some(permit),
                    }))),
                },
            );
        }
        let mut pending = PendingCall {
            retain_reply: false,
            state: self.state.clone(),
            xid,
            token,
        };
        // Keep the completion receiver outside both deadlines. Dropping a timed
        // operation must never discard the destination's return channel.
        let operation = async {
            let write = async {
                tokio::select! {
                    biased;
                    () = self.wait_closed() => Err(self.state.replies().closed.clone().unwrap_or(RuntimeError::ConnectionClosed)),
                    result = self.write_segmented_frame(&segments) => result,
                }
            };
            match self.write_timeout {
                Some(duration) => timeout(duration, write).await.map_err(|_| {
                    RuntimeError::Transport(format!("write timeout after {duration:?}"))
                })??,
                None => write.await?,
            }
            drop(segments);
            pending.retain_reply = true;
            Ok((&mut response)
                .await
                .expect("owned receive target must return its owner"))
        };
        let result = match options.effective_timeout(self.default_call_timeout) {
            Some(duration) => timeout(duration, operation).await.unwrap_or_else(|_| {
                Err(RuntimeError::Transport(format!(
                    "call timeout after {duration:?}"
                )))
            }),
            None => operation.await,
        };
        match result {
            Ok(outcome) => outcome,
            Err(error) => {
                let unsent = if !pending.retain_reply {
                    let mut replies = self.state.replies();
                    if replies.pending.get(&xid).is_some_and(|entry| {
                        Arc::ptr_eq(&entry.token, &pending.token) && entry.sender.is_some()
                    }) {
                        replies.pending.remove(&xid).and_then(|entry| entry.sender)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(target) = unsent {
                    target.fail(error.clone());
                } else {
                    // Includes an active read: abort requests cleanup, and the
                    // target's Drop returns its owner only once the read stops.
                    self.close();
                }
                let mut outcome = response
                    .await
                    .expect("owned receive target must return its owner");
                outcome.result = Err(error);
                outcome
            }
        }
    }

    async fn write_segmented_frame(&self, segments: &[Bytes]) -> Result<(), RuntimeError> {
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
        let result = async {
            write_segments(writer, segments).await?;
            tokio::io::AsyncWriteExt::flush(writer)
                .await
                .map_err(|error| RuntimeError::Transport(error.to_string()))
        }
        .await;
        match &result {
            Ok(()) => frame.complete = true,
            Err(error) => self.state.fail(error.clone()),
        }
        result
    }
}

// Only envelope/control bytes are assembled. Successful result bytes are never
// passed through RpcMessage::decode (which would copy the payload).
async fn reply_header<R: AsyncRead + Unpin>(
    record: &mut RecordReader<'_, R>,
    xid: [u8; 4],
) -> Result<RpcMessage, RuntimeError> {
    struct Envelope {
        bytes: [u8; 432],
        length: usize,
    }
    let mut bytes = Envelope {
        bytes: [0; 432],
        length: 4,
    };
    bytes.bytes[..4].copy_from_slice(&xid);
    async fn word<R: AsyncRead + Unpin>(
        record: &mut RecordReader<'_, R>,
        bytes: &mut Envelope,
    ) -> Result<u32, RuntimeError> {
        let end = bytes.length + 4;
        record
            .read_exact(&mut bytes.bytes[bytes.length..end])
            .await?;
        let result = u32::from_be_bytes(
            bytes.bytes[bytes.length..end]
                .try_into()
                .expect("word width"),
        );
        bytes.length = end;
        Ok(result)
    }
    match word(record, &mut bytes).await? {
        1 => {}
        0 => return Err(RuntimeError::UnexpectedCallMessage),
        other => {
            return Err(RuntimeError::Wire(crate::WireError::InvalidMessageType(
                other,
            )));
        }
    }
    let reply = word(record, &mut bytes).await?;
    match reply {
        0 => {
            word(record, &mut bytes).await?; // verifier flavor
            let len = word(record, &mut bytes).await? as usize;
            if len > onc_rpc_wire::MAX_AUTH_BYTES {
                return Err(RuntimeError::Wire(crate::WireError::AuthBodyTooLong(len)));
            }
            let padded = (len + 3) & !3;
            let start = bytes.length;
            bytes.length += padded;
            record
                .read_exact(&mut bytes.bytes[start..bytes.length])
                .await?;
            if word(record, &mut bytes).await? == 2 {
                word(record, &mut bytes).await?;
                word(record, &mut bytes).await?;
            }
        }
        1 => {
            let reject = word(record, &mut bytes).await?;
            if reject == 0 {
                word(record, &mut bytes).await?;
                word(record, &mut bytes).await?;
            } else if reject == 1 {
                word(record, &mut bytes).await?;
            }
        }
        _ => {}
    }
    RpcMessage::decode(&bytes.bytes[..bytes.length]).map_err(RuntimeError::Wire)
}

async fn drain<R: AsyncRead + Unpin>(
    record: &mut RecordReader<'_, R>,
) -> Result<usize, RuntimeError> {
    let mut scratch = [0; 4096];
    let mut total = 0;
    loop {
        let count = record.read_some(&mut scratch).await?;
        if count == 0 {
            return Ok(total);
        }
        total += count;
    }
}

pub(super) async fn receive_record<R: AsyncRead + Unpin>(
    reader: &mut R,
    owner: &Weak<State>,
    limits: RecordLimits,
) -> Result<(), RuntimeError> {
    let mut record = RecordReader::new(reader, limits)?;
    let mut xid_bytes = [0; 4];
    record.read_exact(&mut xid_bytes).await?;
    let xid = Xid(u32::from_be_bytes(xid_bytes));
    let Some(state) = owner.upgrade() else {
        return Err(RuntimeError::ConnectionClosed);
    };
    let (target, token) = {
        let mut replies = state.replies();
        match replies.pending.get_mut(&xid) {
            Some(pending) => (pending.sender.take(), Some(pending.token.clone())),
            None => (None, None),
        }
    };
    // Do not retain strong connection ownership while blocked on the peer.
    drop(state);
    let result = match target {
        Some(ResponseSender::Buffered { sender, permit }) => {
            let mut bytes = BytesMut::from(xid_bytes.as_slice());
            let result = async {
                loop {
                    let start = bytes.len();
                    bytes.resize(start + 4096, 0);
                    let count = record.read_some(&mut bytes[start..]).await?;
                    bytes.truncate(start + count);
                    if count == 0 {
                        break;
                    }
                }
                let message = RpcMessage::decode(&bytes).map_err(RuntimeError::Wire)?;
                if matches!(message.body, MessageBody::Reply(_)) {
                    Ok(message)
                } else {
                    Err(RuntimeError::UnexpectedCallMessage)
                }
            }
            .await;
            let failure = result.as_ref().err().cloned();
            let _ = sender.send(result);
            drop(permit);
            failure.map_or(Ok(()), Err)
        }
        Some(ResponseSender::Segmented(mut target)) => {
            let received: Result<(RpcMessage, usize), RuntimeError> = async {
                let header = reply_header(&mut record, xid_bytes).await?;
                let success = matches!(
                    header.body,
                    MessageBody::Reply(crate::ReplyBody::Accepted(crate::AcceptedReply {
                        status: crate::AcceptedStatus::Success(_),
                        ..
                    }))
                );
                let mut length = 0;
                if success {
                    loop {
                        let count = {
                            let buffer = target.buffer()?;
                            if buffer.is_empty() {
                                break;
                            }
                            record.read_some(buffer).await?
                        };
                        if count == 0 {
                            return Ok((header, length));
                        }
                        length += count;
                        target.advance(count)?;
                    }
                }
                let mut probe = [0];
                if record.read_some(&mut probe).await? != 0 {
                    let extra = 1 + drain(&mut record).await?;
                    if !success {
                        return Err(RuntimeError::Wire(crate::WireError::TrailingBytes(extra)));
                    }
                    return Ok((header, usize::MAX));
                }
                Ok((header, length))
            }
            .await;
            match received {
                Ok((header, length)) if length != usize::MAX => {
                    target.finish(Ok(header));
                    Ok(())
                }
                Ok((_, _)) => {
                    target.finish(Err(RuntimeError::Transport(
                        "RPC reply exceeds caller buffer capacity".into(),
                    )));
                    Ok(())
                }
                Err(error) => {
                    target.finish(Err(error.clone()));
                    Err(error)
                }
            }
        }
        None => drain(&mut record).await.map(|_| ()),
    };
    if let Some(state) = owner.upgrade() {
        let mut replies = state.replies();
        if let Some(token) = token {
            if replies
                .pending
                .get(&xid)
                .is_some_and(|p| Arc::ptr_eq(&p.token, &token))
            {
                replies.pending.remove(&xid);
            }
        }
    }
    result
}
