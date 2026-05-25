//! Authentication helpers for ONC RPC.

use bytes::Bytes;
use onc_rpc_wire::{AuthFlavor as WireAuthFlavor, OpaqueAuth, WireError};
use onc_rpc_xdr::{XdrDecode, XdrEncode, XdrError};
use thiserror::Error;

const AUTH_SYS_MAX_MACHINE_NAME: usize = 255;
const AUTH_SYS_MAX_GIDS: usize = 16;

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

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("machine name exceeds AUTH_SYS limits")]
    MachineNameTooLong,
    #[error("too many auxiliary gids for AUTH_SYS")]
    TooManyAuxiliaryGroups,
    #[error("unsupported auth flavor: {0:?}")]
    UnsupportedFlavor(WireAuthFlavor),
    #[error("wire auth failure: {0}")]
    Wire(#[from] WireError),
    #[error("xdr failure: {0}")]
    Xdr(#[from] XdrError),
}

impl AuthSys {
    pub fn validate(&self) -> Result<(), AuthError> {
        if self.machine_name.len() > AUTH_SYS_MAX_MACHINE_NAME {
            return Err(AuthError::MachineNameTooLong);
        }
        if self.gids.len() > AUTH_SYS_MAX_GIDS {
            return Err(AuthError::TooManyAuxiliaryGroups);
        }
        Ok(())
    }

    pub fn to_opaque_auth(&self) -> Result<OpaqueAuth, AuthError> {
        self.validate()?;
        let mut encoded = bytes::BytesMut::new();
        self.stamp.encode_xdr(&mut encoded)?;
        self.machine_name.encode_xdr(&mut encoded)?;
        self.uid.encode_xdr(&mut encoded)?;
        self.gid.encode_xdr(&mut encoded)?;
        self.gids.encode_xdr(&mut encoded)?;
        Ok(OpaqueAuth::new(WireAuthFlavor::Sys, encoded.freeze())?)
    }

    pub fn from_opaque_auth(auth: &OpaqueAuth) -> Result<Self, AuthError> {
        if auth.flavor != WireAuthFlavor::Sys {
            return Err(AuthError::UnsupportedFlavor(auth.flavor));
        }

        let mut input = auth.body.as_ref();
        let value = Self {
            stamp: u32::decode_xdr(&mut input)?,
            machine_name: String::decode_xdr(&mut input)?,
            uid: u32::decode_xdr(&mut input)?,
            gid: u32::decode_xdr(&mut input)?,
            gids: Vec::<u32>::decode_xdr(&mut input)?,
        };
        if !input.is_empty() {
            return Err(AuthError::Xdr(XdrError::TrailingBytes(input.len())));
        }
        value.validate()?;
        Ok(value)
    }
}

impl AuthFlavor {
    pub fn to_opaque_auth(&self) -> Result<OpaqueAuth, AuthError> {
        match self {
            Self::None => Ok(OpaqueAuth::none()),
            Self::Sys(value) => value.to_opaque_auth(),
        }
    }

    pub fn from_opaque_auth(auth: &OpaqueAuth) -> Result<Self, AuthError> {
        match auth.flavor {
            WireAuthFlavor::None => Ok(Self::None),
            WireAuthFlavor::Sys => Ok(Self::Sys(AuthSys::from_opaque_auth(auth)?)),
            other => Err(AuthError::UnsupportedFlavor(other)),
        }
    }
}

pub fn auth_none() -> OpaqueAuth {
    OpaqueAuth::none()
}

pub fn auth_sys(
    stamp: u32,
    machine_name: impl Into<String>,
    uid: u32,
    gid: u32,
    gids: Vec<u32>,
) -> Result<OpaqueAuth, AuthError> {
    AuthSys {
        stamp,
        machine_name: machine_name.into(),
        uid,
        gid,
        gids,
    }
    .to_opaque_auth()
}

pub fn decode_auth(auth: &OpaqueAuth) -> Result<AuthFlavor, AuthError> {
    AuthFlavor::from_opaque_auth(auth)
}

pub fn auth_none_verifier() -> OpaqueAuth {
    OpaqueAuth::none()
}

pub fn auth_body(auth: &OpaqueAuth) -> Bytes {
    auth.body.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_none_round_trips() {
        let auth = auth_none();
        assert_eq!(
            decode_auth(&auth).expect("decode should succeed"),
            AuthFlavor::None
        );
    }

    #[test]
    fn auth_sys_round_trips() {
        let source = AuthSys {
            stamp: 7,
            machine_name: "test-host".into(),
            uid: 1000,
            gid: 100,
            gids: vec![101, 102],
        };

        let auth = source.to_opaque_auth().expect("encode should succeed");
        assert_eq!(auth.flavor, WireAuthFlavor::Sys);
        let decoded = AuthSys::from_opaque_auth(&auth).expect("decode should succeed");
        assert_eq!(decoded, source);
        assert_eq!(
            decode_auth(&auth).expect("decode auth should succeed"),
            AuthFlavor::Sys(source)
        );
    }

    #[test]
    fn auth_sys_rejects_too_long_machine_name() {
        let value = AuthSys {
            stamp: 1,
            machine_name: "a".repeat(AUTH_SYS_MAX_MACHINE_NAME + 1),
            uid: 1,
            gid: 1,
            gids: vec![],
        };

        let error = value.to_opaque_auth().expect_err("validation must fail");
        assert_eq!(error, AuthError::MachineNameTooLong);
    }

    #[test]
    fn auth_sys_rejects_too_many_groups() {
        let value = AuthSys {
            stamp: 1,
            machine_name: "host".into(),
            uid: 1,
            gid: 1,
            gids: (0..=AUTH_SYS_MAX_GIDS as u32).collect(),
        };

        let error = value.to_opaque_auth().expect_err("validation must fail");
        assert_eq!(error, AuthError::TooManyAuxiliaryGroups);
    }

    #[test]
    fn unsupported_auth_flavor_is_rejected() {
        let auth = OpaqueAuth::new(WireAuthFlavor::Short, Bytes::new()).expect("valid auth");
        let error = decode_auth(&auth).expect_err("unsupported flavor must fail");
        assert_eq!(error, AuthError::UnsupportedFlavor(WireAuthFlavor::Short));
    }
}
