//! Authentication helpers for ONC RPC.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSys {
    pub stamp: u32,
    pub machine_name: String,
    pub uid: u32,
    pub gid: u32,
    pub gids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFlavor {
    None,
    Sys(AuthSys),
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("machine name exceeds AUTH_SYS limits")]
    MachineNameTooLong,
    #[error("too many auxiliary gids for AUTH_SYS")]
    TooManyAuxiliaryGroups,
}

impl AuthSys {
    pub fn validate(&self) -> Result<(), AuthError> {
        if self.machine_name.len() > 255 {
            return Err(AuthError::MachineNameTooLong);
        }
        if self.gids.len() > 16 {
            return Err(AuthError::TooManyAuxiliaryGroups);
        }
        Ok(())
    }
}
