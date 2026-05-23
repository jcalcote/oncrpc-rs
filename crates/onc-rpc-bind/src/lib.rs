//! Optional rpcbind / portmap support.

use thiserror::Error;

pub const PORTMAP_PORT: u16 = 111;

#[derive(Debug, Error)]
pub enum BindError {
    #[error("rpcbind support is not implemented yet")]
    NotImplemented,
}

pub fn lookup_port(_program: u32, _version: u32) -> Result<u16, BindError> {
    Err(BindError::NotImplemented)
}
