//! TLS/STARTTLS integration points for ONC RPC.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    Disabled,
    Allowed,
    Required,
    MutualAuthRequired,
}

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("starttls support is not implemented yet")]
    StartTlsNotImplemented,
}
