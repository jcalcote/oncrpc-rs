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
    pub header: RpcMessage,
    pub payload: Vec<Bytes>,
}

impl SegmentedRequest {
    fn encoded_segments(&self) -> Result<Vec<Bytes>, RuntimeError> {
        match &self.header.body {
            MessageBody::Call(call) if call.payload.is_empty() => {}
            _ => {
                return Err(RuntimeError::Transport(
                    "segmented request requires an empty call envelope".into(),
                ));
            }
        }
        let header = self.header.encode().map_err(RuntimeError::Wire)?;
        self.payload
            .iter()
            .try_fold(header.len(), |n, segment| n.checked_add(segment.len()))
            .filter(|n| *n <= crate::MAX_FRAGMENT_LEN as usize)
            .ok_or_else(|| {
                RuntimeError::Transport("segmented request exceeds RPC fragment limit".into())
            })?;
        let mut segments = Vec::with_capacity(self.payload.len() + 1);
        segments.push(header);
        segments.extend(self.payload.iter().cloned());
        Ok(segments)
    }
}

/// A decoded envelope and caller-owned buffers filled with its XDR results.
///
/// Only the first `payload_len` bytes across `buffers` were received. Successful
/// RPC envelopes carry an empty `Success` payload: result bytes live exclusively
/// in these buffers. A remote reject is returned as its original RPC envelope.
#[derive(Debug)]
pub struct SegmentedReply<B> {
    pub header: RpcMessage,
    pub buffers: Vec<B>,
    pub payload_len: usize,
}

pub(super) trait ReceiveTarget: Send {
    fn segment_count(&self) -> usize;
    fn segment_mut(&mut self, index: usize) -> &mut [u8];
    fn finish(self: Box<Self>, header: Result<RpcMessage, RuntimeError>, length: usize);
}
struct Target<B> {
    buffers: Vec<B>,
    sender: oneshot::Sender<Result<SegmentedReply<B>, RuntimeError>>,
}
impl<B: AsMut<[u8]> + Send + 'static> ReceiveTarget for Target<B> {
    fn segment_count(&self) -> usize {
        self.buffers.len()
    }
    fn segment_mut(&mut self, index: usize) -> &mut [u8] {
        self.buffers[index].as_mut()
    }
    fn finish(self: Box<Self>, header: Result<RpcMessage, RuntimeError>, length: usize) {
        let Self { buffers, sender } = *self;
        let _ = sender.send(header.map(|header| SegmentedReply {
            header,
            buffers,
            payload_len: length,
        }));
    }
}

impl OwnedRpcTransport {
    /// Send scatter/gather arguments and receive directly into owned buffers.
    ///
    /// Cancelling a fully sent call leaves its XID and buffers owned by the
    /// connection until its reply is consumed or the connection is closed.
    /// Cancellation during a frame write retires the connection. Dropping the
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
        let segments = request.encoded_segments()?;
        let operation = self.call_segmented_inner(request.header.xid, segments, buffers);
        match options.effective_timeout(self.default_call_timeout) {
            Some(duration) => timeout(duration, operation)
                .await
                .map_err(|_| RuntimeError::Transport(format!("call timeout after {duration:?}")))?,
            None => operation.await,
        }
    }

    async fn call_segmented_inner<B: AsMut<[u8]> + Send + 'static>(
        &self,
        xid: Xid,
        segments: Vec<Bytes>,
        buffers: Vec<B>,
    ) -> Result<SegmentedReply<B>, RuntimeError> {
        let (sender, receiver) = oneshot::channel();
        let token = Arc::new(());
        {
            let mut replies = self.state.replies();
            if let Some(error) = &replies.closed {
                return Err(error.clone());
            }
            if replies.pending.contains_key(&xid) {
                return Err(RuntimeError::Transport(
                    "duplicate in-flight RPC XID".into(),
                ));
            }
            replies.pending.insert(
                xid,
                Pending {
                    token: token.clone(),
                    sender: Some(ResponseSender::Segmented(Box::new(Target {
                        buffers,
                        sender,
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
        // Preserve the outstanding XID even if the caller times out: a late
        // reply must not be mistaken for a new operation using that XID.
        pending.retain_reply = true;
        receiver.await.map_err(|_| RuntimeError::ConnectionClosed)?
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
        Some(ResponseSender::Buffered(sender)) => {
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
                    for index in 0..target.segment_count() {
                        let segment = target.segment_mut(index);
                        let mut offset = 0;
                        while offset < segment.len() {
                            let count = record.read_some(&mut segment[offset..]).await?;
                            if count == 0 {
                                return Ok((header, length));
                            }
                            offset += count;
                            length += count;
                        }
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
                    target.finish(Ok(header), length);
                    Ok(())
                }
                Ok((_, _)) => {
                    target.finish(
                        Err(RuntimeError::Transport(
                            "RPC reply exceeds caller buffer capacity".into(),
                        )),
                        0,
                    );
                    Ok(())
                }
                Err(error) => {
                    target.finish(Err(error.clone()), 0);
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
