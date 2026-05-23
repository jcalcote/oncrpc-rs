//! Low-level ONC RPC wire primitives owned by this workspace.

use bytes::Bytes;
use onc_rpc_auth::AuthFlavor;
use thiserror::Error;

pub const RPC_VERSION_2: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Xid(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramVersion {
    pub program: u32,
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Procedure(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueAuth {
    pub flavor: AuthFlavor,
    pub body: Bytes,
}

impl OpaqueAuth {
    pub fn none() -> Self {
        Self {
            flavor: AuthFlavor::None,
            body: Bytes::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordMarker {
    pub last_fragment: bool,
    pub payload_len: u32,
}

impl RecordMarker {
    pub fn new(payload_len: u32, last_fragment: bool) -> Self {
        Self {
            last_fragment,
            payload_len,
        }
    }

    pub fn encode(self) -> u32 {
        let final_fragment = if self.last_fragment { 1_u32 << 31 } else { 0 };
        final_fragment | (self.payload_len & 0x7fff_ffff)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallBody {
    pub xid: Xid,
    pub rpc_version: u32,
    pub program: ProgramVersion,
    pub procedure: Procedure,
    pub credentials: OpaqueAuth,
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

impl CallBody {
    pub fn new(
        xid: Xid,
        program: ProgramVersion,
        procedure: Procedure,
        credentials: OpaqueAuth,
        verifier: OpaqueAuth,
        payload: Bytes,
    ) -> Self {
        Self {
            xid,
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
pub struct ReplyBody {
    pub xid: Xid,
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

#[derive(Debug, Error)]
pub enum WireError {
    #[error("fragmented tcp record reassembly is not implemented yet")]
    FragmentationNotImplemented,
    #[error("rpc message encoding is not implemented yet")]
    EncodingNotImplemented,
    #[error("rpc message decoding is not implemented yet")]
    DecodingNotImplemented,
}
