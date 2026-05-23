//! Runtime primitives for ONC RPC over TCP.

use onc_rpc_auth::AuthFlavor;
use onc_rpc_wire::RecordMarker;
use std::net::SocketAddr;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub remote_addr: SocketAddr,
    pub local_addr: Option<SocketAddr>,
    pub connect_timeout: Duration,
    pub service_name: Option<String>,
    pub auth: AuthFlavor,
}

impl ClientConfig {
    pub fn new(remote_addr: SocketAddr, auth: AuthFlavor) -> Self {
        Self {
            remote_addr,
            local_addr: None,
            connect_timeout: Duration::from_secs(30),
            service_name: None,
            auth,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Xid(pub u32);

#[derive(Debug, Clone)]
pub struct Reply {
    pub xid: Xid,
    pub payload: Vec<u8>,
    pub marker: RecordMarker,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("tcp record fragmentation support is not implemented yet")]
    FragmentationNotImplemented,
    #[error("transport failure: {0}")]
    Transport(String),
}

pub struct Client {
    config: ClientConfig,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }
}
