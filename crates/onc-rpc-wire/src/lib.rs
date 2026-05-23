//! Low-level ONC RPC v2 wire primitives owned by this workspace.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::convert::TryFrom;
use thiserror::Error;

pub const RPC_VERSION_2: u32 = 2;
pub const MAX_AUTH_BYTES: usize = 400;
pub const MAX_FRAGMENT_LEN: u32 = 0x7fff_ffff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Xid(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramVersion {
    pub program: u32,
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Procedure(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionRange {
    pub low: u32,
    pub high: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFlavor {
    None,
    Sys,
    Short,
    Dh,
    RpcSecGss,
    Unknown(u32),
}

impl AuthFlavor {
    pub fn number(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Sys => 1,
            Self::Short => 2,
            Self::Dh => 3,
            Self::RpcSecGss => 6,
            Self::Unknown(value) => value,
        }
    }
}

impl From<u32> for AuthFlavor {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Sys,
            2 => Self::Short,
            3 => Self::Dh,
            6 => Self::RpcSecGss,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueAuth {
    pub flavor: AuthFlavor,
    pub body: Bytes,
}

impl OpaqueAuth {
    pub fn new(flavor: AuthFlavor, body: Bytes) -> Result<Self, WireError> {
        validate_auth_len(body.len())?;
        Ok(Self { flavor, body })
    }

    pub fn none() -> Self {
        Self {
            flavor: AuthFlavor::None,
            body: Bytes::new(),
        }
    }

    fn encode_into(&self, output: &mut BytesMut) -> Result<(), WireError> {
        validate_auth_len(self.body.len())?;
        output.put_u32(self.flavor.number());
        output.put_u32(self.body.len() as u32);
        output.extend_from_slice(&self.body);
        pad_to_xdr_alignment(output, self.body.len());
        Ok(())
    }

    fn decode(input: &mut &[u8]) -> Result<Self, WireError> {
        let flavor = AuthFlavor::from(read_u32(input)?);
        let body_len = read_u32(input)? as usize;
        validate_auth_len(body_len)?;
        let body = read_opaque(input, body_len)?;

        Ok(Self { flavor, body })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordMarker {
    pub last_fragment: bool,
    pub payload_len: u32,
}

impl RecordMarker {
    pub fn new(payload_len: u32, last_fragment: bool) -> Result<Self, WireError> {
        if payload_len > MAX_FRAGMENT_LEN {
            return Err(WireError::FragmentLengthTooLarge(payload_len));
        }

        Ok(Self {
            last_fragment,
            payload_len,
        })
    }

    pub fn encode(self) -> u32 {
        let final_fragment = if self.last_fragment { 1_u32 << 31 } else { 0 };
        final_fragment | self.payload_len
    }

    pub fn encode_bytes(self) -> [u8; 4] {
        self.encode().to_be_bytes()
    }

    pub fn decode(word: u32) -> Self {
        Self {
            last_fragment: (word & (1_u32 << 31)) != 0,
            payload_len: word & MAX_FRAGMENT_LEN,
        }
    }

    pub fn decode_bytes(header: [u8; 4]) -> Self {
        Self::decode(u32::from_be_bytes(header))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordRead {
    Incomplete,
    Complete {
        data: Bytes,
        consumed: usize,
        fragments: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Call,
    Reply,
}

impl MessageType {
    fn number(self) -> u32 {
        match self {
            Self::Call => 0,
            Self::Reply => 1,
        }
    }
}

impl TryFrom<u32> for MessageType {
    type Error = WireError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Call),
            1 => Ok(Self::Reply),
            other => Err(WireError::InvalidMessageType(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyStat {
    MessageAccepted,
    MessageDenied,
}

impl ReplyStat {
    fn number(self) -> u32 {
        match self {
            Self::MessageAccepted => 0,
            Self::MessageDenied => 1,
        }
    }
}

impl TryFrom<u32> for ReplyStat {
    type Error = WireError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::MessageAccepted),
            1 => Ok(Self::MessageDenied),
            other => Err(WireError::InvalidReplyStat(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptStat {
    Success,
    ProgramUnavailable,
    ProgramMismatch,
    ProcedureUnavailable,
    GarbageArgs,
    SystemError,
}

impl AcceptStat {
    fn number(self) -> u32 {
        match self {
            Self::Success => 0,
            Self::ProgramUnavailable => 1,
            Self::ProgramMismatch => 2,
            Self::ProcedureUnavailable => 3,
            Self::GarbageArgs => 4,
            Self::SystemError => 5,
        }
    }
}

impl TryFrom<u32> for AcceptStat {
    type Error = WireError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Success),
            1 => Ok(Self::ProgramUnavailable),
            2 => Ok(Self::ProgramMismatch),
            3 => Ok(Self::ProcedureUnavailable),
            4 => Ok(Self::GarbageArgs),
            5 => Ok(Self::SystemError),
            other => Err(WireError::InvalidAcceptStat(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectStat {
    RpcMismatch,
    AuthError,
}

impl RejectStat {
    fn number(self) -> u32 {
        match self {
            Self::RpcMismatch => 0,
            Self::AuthError => 1,
        }
    }
}

impl TryFrom<u32> for RejectStat {
    type Error = WireError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::RpcMismatch),
            1 => Ok(Self::AuthError),
            other => Err(WireError::InvalidRejectStat(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStat {
    Ok,
    BadCred,
    RejectedCred,
    BadVerf,
    RejectedVerf,
    TooWeak,
    InvalidResp,
    Failed,
    KerbGeneric,
    TimeExpire,
    TicketFile,
    Decode,
    NetAddr,
    RpcSecGssCredProblem,
    RpcSecGssCtxProblem,
    Unknown(u32),
}

impl AuthStat {
    fn number(self) -> u32 {
        match self {
            Self::Ok => 0,
            Self::BadCred => 1,
            Self::RejectedCred => 2,
            Self::BadVerf => 3,
            Self::RejectedVerf => 4,
            Self::TooWeak => 5,
            Self::InvalidResp => 6,
            Self::Failed => 7,
            Self::KerbGeneric => 8,
            Self::TimeExpire => 9,
            Self::TicketFile => 10,
            Self::Decode => 11,
            Self::NetAddr => 12,
            Self::RpcSecGssCredProblem => 13,
            Self::RpcSecGssCtxProblem => 14,
            Self::Unknown(value) => value,
        }
    }
}

impl From<u32> for AuthStat {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::Ok,
            1 => Self::BadCred,
            2 => Self::RejectedCred,
            3 => Self::BadVerf,
            4 => Self::RejectedVerf,
            5 => Self::TooWeak,
            6 => Self::InvalidResp,
            7 => Self::Failed,
            8 => Self::KerbGeneric,
            9 => Self::TimeExpire,
            10 => Self::TicketFile,
            11 => Self::Decode,
            12 => Self::NetAddr,
            13 => Self::RpcSecGssCredProblem,
            14 => Self::RpcSecGssCtxProblem,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallBody {
    pub rpc_version: u32,
    pub program: ProgramVersion,
    pub procedure: Procedure,
    pub credentials: OpaqueAuth,
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

impl CallBody {
    pub fn new(
        program: ProgramVersion,
        procedure: Procedure,
        credentials: OpaqueAuth,
        verifier: OpaqueAuth,
        payload: Bytes,
    ) -> Self {
        Self {
            rpc_version: RPC_VERSION_2,
            program,
            procedure,
            credentials,
            verifier,
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptedStatus {
    Success(Bytes),
    ProgramUnavailable,
    ProgramMismatch(VersionRange),
    ProcedureUnavailable,
    GarbageArgs,
    SystemError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedReply {
    pub verifier: OpaqueAuth,
    pub status: AcceptedStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedReply {
    RpcMismatch(VersionRange),
    AuthError(AuthStat),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyBody {
    Accepted(AcceptedReply),
    Denied(RejectedReply),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageBody {
    Call(CallBody),
    Reply(ReplyBody),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcMessage {
    pub xid: Xid,
    pub body: MessageBody,
}

impl RpcMessage {
    pub fn encode(&self) -> Result<Bytes, WireError> {
        let mut output = BytesMut::new();
        output.put_u32(self.xid.0);

        match &self.body {
            MessageBody::Call(call) => {
                output.put_u32(MessageType::Call.number());
                output.put_u32(call.rpc_version);
                output.put_u32(call.program.program);
                output.put_u32(call.program.version);
                output.put_u32(call.procedure.0);
                call.credentials.encode_into(&mut output)?;
                call.verifier.encode_into(&mut output)?;
                output.extend_from_slice(&call.payload);
            }
            MessageBody::Reply(reply) => {
                output.put_u32(MessageType::Reply.number());
                encode_reply_body(reply, &mut output)?;
            }
        }

        Ok(output.freeze())
    }

    pub fn decode(input: &[u8]) -> Result<Self, WireError> {
        let mut input = input;
        let xid = Xid(read_u32(&mut input)?);
        let message_type = MessageType::try_from(read_u32(&mut input)?)?;

        let body = match message_type {
            MessageType::Call => MessageBody::Call(decode_call_body(&mut input)?),
            MessageType::Reply => MessageBody::Reply(decode_reply_body(&mut input)?),
        };

        Ok(Self { xid, body })
    }
}

pub fn fragment_record(data: &[u8], max_fragment_len: u32) -> Result<Vec<Bytes>, WireError> {
    if max_fragment_len == 0 {
        return Err(WireError::ZeroFragmentLength);
    }
    if max_fragment_len > MAX_FRAGMENT_LEN {
        return Err(WireError::FragmentLengthTooLarge(max_fragment_len));
    }

    let chunk_len = max_fragment_len as usize;
    let fragment_count = data.len().div_ceil(chunk_len).max(1);
    let mut fragments = Vec::with_capacity(fragment_count);

    if data.is_empty() {
        let marker = RecordMarker::new(0, true)?;
        fragments.push(Bytes::copy_from_slice(&marker.encode_bytes()));
        return Ok(fragments);
    }

    for (idx, chunk) in data.chunks(chunk_len).enumerate() {
        let marker = RecordMarker::new(chunk.len() as u32, idx + 1 == fragment_count)?;
        let mut fragment = BytesMut::with_capacity(4 + chunk.len());
        fragment.extend_from_slice(&marker.encode_bytes());
        fragment.extend_from_slice(chunk);
        fragments.push(fragment.freeze());
    }

    Ok(fragments)
}

pub fn read_record(input: &[u8]) -> Result<RecordRead, WireError> {
    let mut offset = 0_usize;
    let mut fragments = 0_usize;
    let mut record = BytesMut::new();

    loop {
        let remaining = &input[offset..];
        if remaining.is_empty() && fragments == 0 {
            return Ok(RecordRead::Incomplete);
        }
        if remaining.len() < 4 {
            return Ok(RecordRead::Incomplete);
        }

        let marker =
            RecordMarker::decode_bytes(remaining[..4].try_into().expect("slice len checked"));
        let fragment_len = marker.payload_len as usize;
        let fragment_end = 4 + fragment_len;

        if remaining.len() < fragment_end {
            return Ok(RecordRead::Incomplete);
        }

        record.extend_from_slice(&remaining[4..fragment_end]);
        offset += fragment_end;
        fragments += 1;

        if marker.last_fragment {
            return Ok(RecordRead::Complete {
                data: record.freeze(),
                consumed: offset,
                fragments,
            });
        }
    }
}

fn decode_call_body(input: &mut &[u8]) -> Result<CallBody, WireError> {
    let rpc_version = read_u32(input)?;
    if rpc_version != RPC_VERSION_2 {
        return Err(WireError::UnsupportedRpcVersion(rpc_version));
    }

    let program = ProgramVersion {
        program: read_u32(input)?,
        version: read_u32(input)?,
    };
    let procedure = Procedure(read_u32(input)?);
    let credentials = OpaqueAuth::decode(input)?;
    let verifier = OpaqueAuth::decode(input)?;
    let payload = Bytes::copy_from_slice(input);
    *input = &[];

    Ok(CallBody {
        rpc_version,
        program,
        procedure,
        credentials,
        verifier,
        payload,
    })
}

fn decode_reply_body(input: &mut &[u8]) -> Result<ReplyBody, WireError> {
    match ReplyStat::try_from(read_u32(input)?)? {
        ReplyStat::MessageAccepted => decode_accepted_reply(input).map(ReplyBody::Accepted),
        ReplyStat::MessageDenied => decode_rejected_reply(input).map(ReplyBody::Denied),
    }
}

fn decode_accepted_reply(input: &mut &[u8]) -> Result<AcceptedReply, WireError> {
    let verifier = OpaqueAuth::decode(input)?;
    let status = match AcceptStat::try_from(read_u32(input)?)? {
        AcceptStat::Success => {
            let payload = Bytes::copy_from_slice(input);
            *input = &[];
            AcceptedStatus::Success(payload)
        }
        AcceptStat::ProgramUnavailable => expect_end(input, AcceptedStatus::ProgramUnavailable)?,
        AcceptStat::ProgramMismatch => {
            let range = VersionRange {
                low: read_u32(input)?,
                high: read_u32(input)?,
            };
            expect_end(input, AcceptedStatus::ProgramMismatch(range))?
        }
        AcceptStat::ProcedureUnavailable => {
            expect_end(input, AcceptedStatus::ProcedureUnavailable)?
        }
        AcceptStat::GarbageArgs => expect_end(input, AcceptedStatus::GarbageArgs)?,
        AcceptStat::SystemError => expect_end(input, AcceptedStatus::SystemError)?,
    };

    Ok(AcceptedReply { verifier, status })
}

fn decode_rejected_reply(input: &mut &[u8]) -> Result<RejectedReply, WireError> {
    match RejectStat::try_from(read_u32(input)?)? {
        RejectStat::RpcMismatch => {
            let range = VersionRange {
                low: read_u32(input)?,
                high: read_u32(input)?,
            };
            expect_end(input, RejectedReply::RpcMismatch(range))
        }
        RejectStat::AuthError => {
            let status = AuthStat::from(read_u32(input)?);
            expect_end(input, RejectedReply::AuthError(status))
        }
    }
}

fn encode_reply_body(reply: &ReplyBody, output: &mut BytesMut) -> Result<(), WireError> {
    match reply {
        ReplyBody::Accepted(accepted) => {
            output.put_u32(ReplyStat::MessageAccepted.number());
            accepted.verifier.encode_into(output)?;
            match &accepted.status {
                AcceptedStatus::Success(results) => {
                    output.put_u32(AcceptStat::Success.number());
                    output.extend_from_slice(results);
                }
                AcceptedStatus::ProgramUnavailable => {
                    output.put_u32(AcceptStat::ProgramUnavailable.number());
                }
                AcceptedStatus::ProgramMismatch(range) => {
                    output.put_u32(AcceptStat::ProgramMismatch.number());
                    output.put_u32(range.low);
                    output.put_u32(range.high);
                }
                AcceptedStatus::ProcedureUnavailable => {
                    output.put_u32(AcceptStat::ProcedureUnavailable.number());
                }
                AcceptedStatus::GarbageArgs => {
                    output.put_u32(AcceptStat::GarbageArgs.number());
                }
                AcceptedStatus::SystemError => {
                    output.put_u32(AcceptStat::SystemError.number());
                }
            }
        }
        ReplyBody::Denied(rejected) => {
            output.put_u32(ReplyStat::MessageDenied.number());
            match rejected {
                RejectedReply::RpcMismatch(range) => {
                    output.put_u32(RejectStat::RpcMismatch.number());
                    output.put_u32(range.low);
                    output.put_u32(range.high);
                }
                RejectedReply::AuthError(status) => {
                    output.put_u32(RejectStat::AuthError.number());
                    output.put_u32(status.number());
                }
            }
        }
    }

    Ok(())
}

fn read_u32(input: &mut &[u8]) -> Result<u32, WireError> {
    if input.len() < 4 {
        return Err(WireError::UnexpectedEof {
            needed: 4,
            remaining: input.len(),
        });
    }

    Ok(input.get_u32())
}

fn read_opaque(input: &mut &[u8], len: usize) -> Result<Bytes, WireError> {
    if input.len() < len {
        return Err(WireError::UnexpectedEof {
            needed: len,
            remaining: input.len(),
        });
    }

    let body = Bytes::copy_from_slice(&input[..len]);
    *input = &input[len..];

    let padding = xdr_padding(len);
    if input.len() < padding {
        return Err(WireError::UnexpectedEof {
            needed: padding,
            remaining: input.len(),
        });
    }
    if input[..padding].iter().any(|byte| *byte != 0) {
        return Err(WireError::NonZeroXdrPadding);
    }
    *input = &input[padding..];

    Ok(body)
}

fn pad_to_xdr_alignment(output: &mut BytesMut, len: usize) {
    let padding = xdr_padding(len);
    for _ in 0..padding {
        output.put_u8(0);
    }
}

fn xdr_padding(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

fn validate_auth_len(len: usize) -> Result<(), WireError> {
    if len > MAX_AUTH_BYTES {
        return Err(WireError::AuthBodyTooLong(len));
    }

    Ok(())
}

fn expect_end<T>(input: &mut &[u8], value: T) -> Result<T, WireError> {
    if input.is_empty() {
        Ok(value)
    } else {
        Err(WireError::TrailingBytes(input.len()))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    #[error("unexpected end of input: need {needed} bytes, have {remaining}")]
    UnexpectedEof { needed: usize, remaining: usize },
    #[error("record fragment length {0} exceeds the 31-bit record marking limit")]
    FragmentLengthTooLarge(u32),
    #[error("record fragment size cannot be zero when fragmenting a non-empty record")]
    ZeroFragmentLength,
    #[error("unsupported rpc version {0}")]
    UnsupportedRpcVersion(u32),
    #[error("invalid message type discriminant {0}")]
    InvalidMessageType(u32),
    #[error("invalid reply status discriminant {0}")]
    InvalidReplyStat(u32),
    #[error("invalid accept status discriminant {0}")]
    InvalidAcceptStat(u32),
    #[error("invalid reject status discriminant {0}")]
    InvalidRejectStat(u32),
    #[error("auth body exceeds RFC 5531 limit of 400 bytes: {0}")]
    AuthBodyTooLong(usize),
    #[error("encountered non-zero XDR padding bytes")]
    NonZeroXdrPadding,
    #[error("unexpected trailing bytes after fixed-width reply branch: {0}")]
    TrailingBytes(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(body: &[u8]) -> OpaqueAuth {
        OpaqueAuth::new(AuthFlavor::None, Bytes::copy_from_slice(body)).expect("valid auth")
    }

    #[test]
    fn record_marker_encodes_and_decodes() {
        let marker = RecordMarker::new(1024, true).expect("valid marker");

        assert_eq!(marker.encode(), 0x8000_0400);
        assert_eq!(
            RecordMarker::decode_bytes(marker.encode_bytes()),
            RecordMarker {
                last_fragment: true,
                payload_len: 1024,
            }
        );
    }

    #[test]
    fn fragment_record_splits_payload_into_multiple_fragments() {
        let data = b"abcdefgh";
        let fragments = fragment_record(data, 3).expect("fragmentation should succeed");

        assert_eq!(fragments.len(), 3);
        assert_eq!(&fragments[0][..4], &[0x00, 0x00, 0x00, 0x03]);
        assert_eq!(&fragments[1][..4], &[0x00, 0x00, 0x00, 0x03]);
        assert_eq!(&fragments[2][..4], &[0x80, 0x00, 0x00, 0x02]);
        assert_eq!(&fragments[0][4..], b"abc");
        assert_eq!(&fragments[1][4..], b"def");
        assert_eq!(&fragments[2][4..], b"gh");
    }

    #[test]
    fn read_record_reassembles_fragmented_message() {
        let mut encoded = Vec::new();
        for fragment in fragment_record(b"abcdefgh", 3).expect("fragmentation should succeed") {
            encoded.extend_from_slice(&fragment);
        }

        let record = read_record(&encoded).expect("record read should succeed");
        assert_eq!(
            record,
            RecordRead::Complete {
                data: Bytes::from_static(b"abcdefgh"),
                consumed: encoded.len(),
                fragments: 3,
            }
        );
    }

    #[test]
    fn read_record_reports_incomplete_header() {
        let record = read_record(&[0x80, 0x00]).expect("partial header is not malformed");
        assert_eq!(record, RecordRead::Incomplete);
    }

    #[test]
    fn read_record_reports_incomplete_fragment_payload() {
        let record = read_record(&[0x80, 0x00, 0x00, 0x04, 0xaa, 0xbb])
            .expect("short payload is not malformed");
        assert_eq!(record, RecordRead::Incomplete);
    }

    #[test]
    fn opaque_auth_rejects_oversized_body() {
        let body = Bytes::from(vec![0_u8; MAX_AUTH_BYTES + 1]);
        let error = OpaqueAuth::new(AuthFlavor::Sys, body).expect_err("body must be rejected");

        assert_eq!(error, WireError::AuthBodyTooLong(MAX_AUTH_BYTES + 1));
    }

    #[test]
    fn rpc_call_round_trips() {
        let message = RpcMessage {
            xid: Xid(0x0102_0304),
            body: MessageBody::Call(CallBody::new(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(1),
                auth(&[]),
                auth(&[0xaa, 0xbb, 0xcc]),
                Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            )),
        };

        let encoded = message.encode().expect("call should encode");
        let decoded = RpcMessage::decode(&encoded).expect("call should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn rpc_reply_round_trips() {
        let message = RpcMessage {
            xid: Xid(42),
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: auth(&[]),
                status: AcceptedStatus::Success(Bytes::from_static(&[0, 1, 2, 3])),
            })),
        };

        let encoded = message.encode().expect("reply should encode");
        let decoded = RpcMessage::decode(&encoded).expect("reply should decode");

        assert_eq!(decoded, message);
    }

    #[test]
    fn rpc_decode_rejects_invalid_message_type() {
        let mut bytes = BytesMut::new();
        bytes.put_u32(7);
        bytes.put_u32(9);

        let error = RpcMessage::decode(&bytes).expect_err("invalid message type must fail");
        assert_eq!(error, WireError::InvalidMessageType(9));
    }

    #[test]
    fn rpc_decode_rejects_invalid_accept_status() {
        let mut bytes = BytesMut::new();
        bytes.put_u32(7);
        bytes.put_u32(MessageType::Reply.number());
        bytes.put_u32(ReplyStat::MessageAccepted.number());
        auth(&[]).encode_into(&mut bytes).expect("verifier encodes");
        bytes.put_u32(88);

        let error = RpcMessage::decode(&bytes).expect_err("invalid accept status must fail");
        assert_eq!(error, WireError::InvalidAcceptStat(88));
    }

    #[test]
    fn rpc_decode_rejects_non_zero_auth_padding() {
        let bytes = [
            0, 0, 0, 1, // xid
            0, 0, 0, 0, // CALL
            0, 0, 0, 2, // rpcvers
            0, 0, 0, 3, // prog
            0, 0, 0, 4, // vers
            0, 0, 0, 5, // proc
            0, 0, 0, 0, // cred flavor
            0, 0, 0, 3, // cred len
            1, 2, 3, 0xff, // cred body + bad pad
            0, 0, 0, 0, // verf flavor
            0, 0, 0, 0, // verf len
        ];

        let error = RpcMessage::decode(&bytes).expect_err("invalid padding must fail");
        assert_eq!(error, WireError::NonZeroXdrPadding);
    }

    #[test]
    fn rpc_decode_rejects_trailing_bytes_on_fixed_reply_branch() {
        let mut bytes = BytesMut::new();
        bytes.put_u32(9);
        bytes.put_u32(MessageType::Reply.number());
        bytes.put_u32(ReplyStat::MessageDenied.number());
        bytes.put_u32(RejectStat::AuthError.number());
        bytes.put_u32(AuthStat::Failed.number());
        bytes.put_u32(0xdead_beef);

        let error = RpcMessage::decode(&bytes).expect_err("trailing bytes must fail");
        assert_eq!(error, WireError::TrailingBytes(4));
    }

    #[test]
    fn rpc_decode_rejects_wrong_rpc_version() {
        let mut bytes = BytesMut::new();
        bytes.put_u32(1);
        bytes.put_u32(MessageType::Call.number());
        bytes.put_u32(99);
        bytes.put_u32(3);
        bytes.put_u32(4);
        bytes.put_u32(5);
        auth(&[]).encode_into(&mut bytes).expect("cred encodes");
        auth(&[]).encode_into(&mut bytes).expect("verifier encodes");

        let error = RpcMessage::decode(&bytes).expect_err("unexpected rpc version must fail");
        assert_eq!(error, WireError::UnsupportedRpcVersion(99));
    }
}
