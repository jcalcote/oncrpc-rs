//! Optional rpcbind / portmap support.

use async_trait::async_trait;
use bytes::BytesMut;
use onc_rpc_runtime::{
    AsyncClient, Client, ClientConfig, Procedure, ProgramVersion, RuntimeError,
    TokioAsyncClientTransport, TokioClientTransport,
};
use onc_rpc_server::{
    TokioAsyncServerTransport, TokioAsyncUdpServerTransport, TokioServerTransport,
    TokioUdpServerTransport,
};
use onc_rpc_xdr::{XdrDecode, XdrEncode, XdrError};
use std::net::{IpAddr, SocketAddr};
use thiserror::Error;

pub const RPCBIND_PORT: u16 = 111;
pub const RPCBIND_PROGRAM: ProgramVersion = ProgramVersion {
    program: 100_000,
    version: 4,
};
pub const RPCBIND_NULL: Procedure = Procedure(0);
pub const RPCBIND_SET: Procedure = Procedure(1);
pub const RPCBIND_UNSET: Procedure = Procedure(2);
pub const RPCBIND_GETADDR: Procedure = Procedure(3);

const DEFAULT_OWNER: &str = "oncrpc-rs";

#[derive(Debug, Error)]
pub enum BindError {
    #[error("runtime failure: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("xdr failure: {0}")]
    Xdr(#[from] XdrError),
    #[error("rpcbind lookup returned no address for program {program}:{version}")]
    NotFound { program: u32, version: u32 },
    #[error("invalid rpcbind universal address: {0}")]
    InvalidUniversalAddress(String),
    #[error("rpcbind operations require a specific local service address, not {0}")]
    UnspecifiedServiceAddress(SocketAddr),
    #[error("server transport failure: {0}")]
    ServerTransport(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcb {
    pub program: u32,
    pub version: u32,
    pub netid: String,
    pub address: String,
    pub owner: String,
}

impl Rpcb {
    pub fn for_tcp(
        program: ProgramVersion,
        service_addr: SocketAddr,
        owner: impl Into<String>,
    ) -> Self {
        Self {
            program: program.program,
            version: program.version,
            netid: tcp_netid(service_addr).to_string(),
            address: universal_addr(service_addr),
            owner: owner.into(),
        }
    }

    pub fn for_udp(
        program: ProgramVersion,
        service_addr: SocketAddr,
        owner: impl Into<String>,
    ) -> Self {
        Self {
            program: program.program,
            version: program.version,
            netid: udp_netid(service_addr).to_string(),
            address: universal_addr(service_addr),
            owner: owner.into(),
        }
    }
}

impl XdrEncode for Rpcb {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        self.program.encode_xdr(output)?;
        self.version.encode_xdr(output)?;
        self.netid.encode_xdr(output)?;
        self.address.encode_xdr(output)?;
        self.owner.encode_xdr(output)?;
        Ok(())
    }
}

impl XdrDecode for Rpcb {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        Ok(Self {
            program: u32::decode_xdr(input)?,
            version: u32::decode_xdr(input)?,
            netid: String::decode_xdr(input)?,
            address: String::decode_xdr(input)?,
            owner: String::decode_xdr(input)?,
        })
    }
}

pub fn rpcbind_addr_for_host(host: IpAddr) -> SocketAddr {
    SocketAddr::new(host, RPCBIND_PORT)
}

pub fn universal_addr(addr: SocketAddr) -> String {
    let [high, low] = addr.port().to_be_bytes();
    format!("{}.{}.{}", addr.ip(), high, low)
}

pub fn parse_universal_addr(value: &str) -> Result<SocketAddr, BindError> {
    let mut parts = value.rsplitn(3, '.');
    let low = parts
        .next()
        .ok_or_else(|| BindError::InvalidUniversalAddress(value.into()))?
        .parse::<u8>()
        .map_err(|_| BindError::InvalidUniversalAddress(value.into()))?;
    let high = parts
        .next()
        .ok_or_else(|| BindError::InvalidUniversalAddress(value.into()))?
        .parse::<u8>()
        .map_err(|_| BindError::InvalidUniversalAddress(value.into()))?;
    let host = parts
        .next()
        .ok_or_else(|| BindError::InvalidUniversalAddress(value.into()))?;
    let ip = host
        .parse::<IpAddr>()
        .map_err(|_| BindError::InvalidUniversalAddress(value.into()))?;
    Ok(SocketAddr::new(ip, u16::from_be_bytes([high, low])))
}

pub fn lookup_port(rpcbind_addr: SocketAddr, program: u32, version: u32) -> Result<u16, BindError> {
    let client = RpcbindClient::connect(rpcbind_addr)?;
    Ok(client
        .lookup_tcp_addr(ProgramVersion { program, version })?
        .port())
}

pub async fn lookup_port_async(
    rpcbind_addr: SocketAddr,
    program: u32,
    version: u32,
) -> Result<u16, BindError> {
    let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
    Ok(client
        .lookup_tcp_addr(ProgramVersion { program, version })
        .await?
        .port())
}

pub fn lookup_udp_port(
    rpcbind_addr: SocketAddr,
    program: u32,
    version: u32,
) -> Result<u16, BindError> {
    let client = RpcbindClient::connect(rpcbind_addr)?;
    Ok(client
        .lookup_udp_addr(ProgramVersion { program, version })?
        .port())
}

pub async fn lookup_udp_port_async(
    rpcbind_addr: SocketAddr,
    program: u32,
    version: u32,
) -> Result<u16, BindError> {
    let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
    Ok(client
        .lookup_udp_addr(ProgramVersion { program, version })
        .await?
        .port())
}

pub fn resolve_client_config(
    rpcbind_addr: SocketAddr,
    program: ProgramVersion,
) -> Result<ClientConfig, BindError> {
    let client = RpcbindClient::connect(rpcbind_addr)?;
    let service_addr = client.lookup_tcp_addr(program)?;
    Ok(ClientConfig::new(service_addr))
}

pub async fn resolve_client_config_async(
    rpcbind_addr: SocketAddr,
    program: ProgramVersion,
) -> Result<ClientConfig, BindError> {
    let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
    let service_addr = client.lookup_tcp_addr(program).await?;
    Ok(ClientConfig::new(service_addr))
}

pub fn resolve_udp_client_config(
    rpcbind_addr: SocketAddr,
    program: ProgramVersion,
) -> Result<ClientConfig, BindError> {
    let client = RpcbindClient::connect(rpcbind_addr)?;
    let service_addr = client.lookup_udp_addr(program)?;
    Ok(ClientConfig::new(service_addr))
}

pub async fn resolve_udp_client_config_async(
    rpcbind_addr: SocketAddr,
    program: ProgramVersion,
) -> Result<ClientConfig, BindError> {
    let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
    let service_addr = client.lookup_udp_addr(program).await?;
    Ok(ClientConfig::new(service_addr))
}

pub struct RpcbindClient {
    client: Client<TokioClientTransport>,
    netid: &'static str,
}

impl RpcbindClient {
    pub fn connect(rpcbind_addr: SocketAddr) -> Result<Self, BindError> {
        let config =
            ClientConfig::new(rpcbind_addr).with_connect_timeout(std::time::Duration::from_secs(5));
        let transport = TokioClientTransport::connect(&config)?;
        Ok(Self {
            client: Client::new(config, transport),
            netid: tcp_netid(rpcbind_addr),
        })
    }

    pub fn lookup_tcp_addr(&self, program: ProgramVersion) -> Result<SocketAddr, BindError> {
        self.lookup_addr(program, self.netid)
    }

    pub fn lookup_udp_addr(&self, program: ProgramVersion) -> Result<SocketAddr, BindError> {
        self.lookup_addr(program, udp_netid(self.client.config().remote_addr))
    }

    fn lookup_addr(
        &self,
        program: ProgramVersion,
        netid: &'static str,
    ) -> Result<SocketAddr, BindError> {
        let request = Rpcb {
            program: program.program,
            version: program.version,
            netid: netid.into(),
            address: String::new(),
            owner: String::new(),
        };
        let result: String = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_GETADDR, &request)?;
        if result.is_empty() {
            return Err(BindError::NotFound {
                program: program.program,
                version: program.version,
            });
        }
        parse_universal_addr(&result)
    }

    pub fn register_tcp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_tcp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_SET, &request)?;
        Ok(response)
    }

    pub fn unregister_tcp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_tcp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_UNSET, &request)?;
        Ok(response)
    }

    pub fn register_udp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_udp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_SET, &request)?;
        Ok(response)
    }

    pub fn unregister_udp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_udp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_UNSET, &request)?;
        Ok(response)
    }
}

pub struct AsyncRpcbindClient {
    client: AsyncClient<TokioAsyncClientTransport>,
    netid: &'static str,
}

impl AsyncRpcbindClient {
    pub async fn connect(rpcbind_addr: SocketAddr) -> Result<Self, BindError> {
        let config =
            ClientConfig::new(rpcbind_addr).with_connect_timeout(std::time::Duration::from_secs(5));
        let transport = TokioAsyncClientTransport::connect(&config).await?;
        Ok(Self {
            client: AsyncClient::new(config, transport),
            netid: tcp_netid(rpcbind_addr),
        })
    }

    pub async fn lookup_tcp_addr(&self, program: ProgramVersion) -> Result<SocketAddr, BindError> {
        self.lookup_addr(program, self.netid).await
    }

    pub async fn lookup_udp_addr(&self, program: ProgramVersion) -> Result<SocketAddr, BindError> {
        self.lookup_addr(program, udp_netid(self.client.config().remote_addr))
            .await
    }

    async fn lookup_addr(
        &self,
        program: ProgramVersion,
        netid: &'static str,
    ) -> Result<SocketAddr, BindError> {
        let request = Rpcb {
            program: program.program,
            version: program.version,
            netid: netid.into(),
            address: String::new(),
            owner: String::new(),
        };
        let result: String = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_GETADDR, &request)
            .await?;
        if result.is_empty() {
            return Err(BindError::NotFound {
                program: program.program,
                version: program.version,
            });
        }
        parse_universal_addr(&result)
    }

    pub async fn register_tcp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_tcp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_SET, &request)
            .await?;
        Ok(response)
    }

    pub async fn unregister_tcp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_tcp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_UNSET, &request)
            .await?;
        Ok(response)
    }

    pub async fn register_udp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_udp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_SET, &request)
            .await?;
        Ok(response)
    }

    pub async fn unregister_udp(
        &self,
        service_addr: SocketAddr,
        program: ProgramVersion,
        owner: impl Into<String>,
    ) -> Result<bool, BindError> {
        let request = Rpcb::for_udp(program, checked_service_addr(service_addr)?, owner);
        let response: bool = self
            .client
            .call_typed(RPCBIND_PROGRAM, RPCBIND_UNSET, &request)
            .await?;
        Ok(response)
    }
}

#[async_trait]
pub trait TokioAsyncServerTransportRpcbindExt {
    async fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    async fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    async fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError>;
}

#[async_trait]
impl TokioAsyncServerTransportRpcbindExt for TokioAsyncServerTransport {
    async fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client
                .register_tcp(
                    service_addr,
                    ProgramVersion {
                        program: program.number,
                        version: program.version,
                    },
                    owner.clone(),
                )
                .await?;
        }

        Ok(())
    }

    async fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client
                .unregister_tcp(
                    service_addr,
                    ProgramVersion {
                        program: program.number,
                        version: program.version,
                    },
                    owner.clone(),
                )
                .await?;
        }

        Ok(())
    }

    async fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError> {
        if !self.config().auto_publish {
            return Ok(false);
        }
        self.publish_rpcbind(rpcbind_addr).await?;
        Ok(true)
    }
}

pub trait TokioServerTransportRpcbindExt {
    fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError>;
}

#[async_trait]
pub trait TokioAsyncUdpServerTransportRpcbindExt {
    async fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    async fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    async fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError>;
}

pub trait TokioUdpServerTransportRpcbindExt {
    fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError>;
    fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError>;
}

impl TokioServerTransportRpcbindExt for TokioServerTransport {
    fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = RpcbindClient::connect(rpcbind_addr)?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client.register_tcp(
                service_addr,
                ProgramVersion {
                    program: program.number,
                    version: program.version,
                },
                owner.clone(),
            )?;
        }

        Ok(())
    }

    fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = RpcbindClient::connect(rpcbind_addr)?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client.unregister_tcp(
                service_addr,
                ProgramVersion {
                    program: program.number,
                    version: program.version,
                },
                owner.clone(),
            )?;
        }

        Ok(())
    }

    fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError> {
        if !self.config().auto_publish {
            return Ok(false);
        }
        self.publish_rpcbind(rpcbind_addr)?;
        Ok(true)
    }
}

#[async_trait]
impl TokioAsyncUdpServerTransportRpcbindExt for TokioAsyncUdpServerTransport {
    async fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client
                .register_udp(
                    service_addr,
                    ProgramVersion {
                        program: program.number,
                        version: program.version,
                    },
                    owner.clone(),
                )
                .await?;
        }

        Ok(())
    }

    async fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = AsyncRpcbindClient::connect(rpcbind_addr).await?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client
                .unregister_udp(
                    service_addr,
                    ProgramVersion {
                        program: program.number,
                        version: program.version,
                    },
                    owner.clone(),
                )
                .await?;
        }

        Ok(())
    }

    async fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError> {
        if !self.config().auto_publish {
            return Ok(false);
        }
        self.publish_rpcbind(rpcbind_addr).await?;
        Ok(true)
    }
}

impl TokioUdpServerTransportRpcbindExt for TokioUdpServerTransport {
    fn publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = RpcbindClient::connect(rpcbind_addr)?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client.register_udp(
                service_addr,
                ProgramVersion {
                    program: program.number,
                    version: program.version,
                },
                owner.clone(),
            )?;
        }

        Ok(())
    }

    fn unpublish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<(), BindError> {
        let service_addr = checked_service_addr(
            self.local_addr()
                .map_err(|err| BindError::ServerTransport(err.to_string()))?,
        )?;
        let client = RpcbindClient::connect(rpcbind_addr)?;
        let owner = self
            .config()
            .service_name
            .clone()
            .unwrap_or_else(|| DEFAULT_OWNER.into());

        for program in self.registered_programs() {
            client.unregister_udp(
                service_addr,
                ProgramVersion {
                    program: program.number,
                    version: program.version,
                },
                owner.clone(),
            )?;
        }

        Ok(())
    }

    fn maybe_publish_rpcbind(&self, rpcbind_addr: SocketAddr) -> Result<bool, BindError> {
        if !self.config().auto_publish {
            return Ok(false);
        }
        self.publish_rpcbind(rpcbind_addr)?;
        Ok(true)
    }
}

fn tcp_netid(addr: SocketAddr) -> &'static str {
    match addr {
        SocketAddr::V4(_) => "tcp",
        SocketAddr::V6(_) => "tcp6",
    }
}

fn udp_netid(addr: SocketAddr) -> &'static str {
    match addr {
        SocketAddr::V4(_) => "udp",
        SocketAddr::V6(_) => "udp6",
    }
}

fn checked_service_addr(addr: SocketAddr) -> Result<SocketAddr, BindError> {
    if addr.ip().is_unspecified() {
        return Err(BindError::UnspecifiedServiceAddress(addr));
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onc_rpc_server::{
        AsyncDispatch, Dispatch, DispatchError, Program, RequestContext, ResponsePayload,
        ServerBuilder, TokioAsyncServerTransport,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct MappingKey {
        program: u32,
        version: u32,
        netid: String,
    }

    #[derive(Clone, Default)]
    struct RpcbindState {
        mappings: Arc<Mutex<HashMap<MappingKey, Rpcb>>>,
    }

    impl RpcbindState {
        fn set(&self, rpcb: Rpcb) -> bool {
            self.mappings
                .lock()
                .expect("mutex poisoned")
                .insert(
                    MappingKey {
                        program: rpcb.program,
                        version: rpcb.version,
                        netid: rpcb.netid.clone(),
                    },
                    rpcb,
                )
                .is_none()
        }

        fn unset(&self, rpcb: &Rpcb) -> bool {
            self.mappings
                .lock()
                .expect("mutex poisoned")
                .remove(&MappingKey {
                    program: rpcb.program,
                    version: rpcb.version,
                    netid: rpcb.netid.clone(),
                })
                .is_some()
        }

        fn getaddr(&self, rpcb: &Rpcb) -> String {
            self.mappings
                .lock()
                .expect("mutex poisoned")
                .get(&MappingKey {
                    program: rpcb.program,
                    version: rpcb.version,
                    netid: rpcb.netid.clone(),
                })
                .map(|value| value.address.clone())
                .unwrap_or_default()
        }
    }

    struct SyncRpcbindDispatch {
        state: RpcbindState,
    }

    impl Dispatch for SyncRpcbindDispatch {
        fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
            match request.procedure {
                RPCBIND_NULL => Ok(ResponsePayload::success(bytes::Bytes::new())),
                RPCBIND_SET => {
                    let value = Rpcb::from_xdr_bytes(&request.payload)
                        .map_err(|_| DispatchError::GarbageArgs)?;
                    Ok(ResponsePayload::success(
                        self.state
                            .set(value)
                            .to_xdr_bytes()
                            .map_err(|_| DispatchError::SystemError)?,
                    ))
                }
                RPCBIND_UNSET => {
                    let value = Rpcb::from_xdr_bytes(&request.payload)
                        .map_err(|_| DispatchError::GarbageArgs)?;
                    Ok(ResponsePayload::success(
                        self.state
                            .unset(&value)
                            .to_xdr_bytes()
                            .map_err(|_| DispatchError::SystemError)?,
                    ))
                }
                RPCBIND_GETADDR => {
                    let value = Rpcb::from_xdr_bytes(&request.payload)
                        .map_err(|_| DispatchError::GarbageArgs)?;
                    Ok(ResponsePayload::success(
                        self.state
                            .getaddr(&value)
                            .to_xdr_bytes()
                            .map_err(|_| DispatchError::SystemError)?,
                    ))
                }
                _ => Err(DispatchError::ProcedureUnavailable),
            }
        }
    }

    struct AsyncRpcbindDispatch {
        state: RpcbindState,
    }

    #[async_trait]
    impl AsyncDispatch for AsyncRpcbindDispatch {
        async fn dispatch(
            &self,
            request: RequestContext,
        ) -> Result<ResponsePayload, DispatchError> {
            SyncRpcbindDispatch {
                state: self.state.clone(),
            }
            .dispatch(request)
        }
    }

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().expect("valid loopback")
    }

    #[test]
    fn universal_address_round_trips_ipv4() {
        let addr: SocketAddr = "127.0.0.1:2049".parse().expect("valid addr");
        let encoded = universal_addr(addr);
        let decoded = parse_universal_addr(&encoded).expect("addr should parse");
        assert_eq!(decoded, addr);
    }

    #[test]
    fn universal_address_round_trips_ipv6() {
        let addr: SocketAddr = "[::1]:2049".parse().expect("valid addr");
        let encoded = universal_addr(addr);
        let decoded = parse_universal_addr(&encoded).expect("addr should parse");
        assert_eq!(decoded, addr);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_client_can_register_lookup_and_unregister() {
        let state = RpcbindState::default();
        let mut server = ServerBuilder::new()
            .with_bind_addr(loopback())
            .build_async();
        server
            .register(
                Program {
                    number: RPCBIND_PROGRAM.program,
                    version: RPCBIND_PROGRAM.version,
                },
                AsyncRpcbindDispatch {
                    state: state.clone(),
                },
            )
            .expect("registration should succeed");
        let transport = TokioAsyncServerTransport::bind(server)
            .await
            .expect("rpcbind should bind");
        let addr = transport.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = transport.serve().await;
        });

        let client = AsyncRpcbindClient::connect(addr)
            .await
            .expect("connect should succeed");
        let service = ProgramVersion {
            program: 200_001,
            version: 7,
        };
        let service_addr: SocketAddr = "127.0.0.1:4040".parse().expect("valid addr");

        assert!(
            client
                .register_tcp(service_addr, service, "owner")
                .await
                .expect("register should succeed")
        );
        assert_eq!(
            client
                .lookup_tcp_addr(service)
                .await
                .expect("lookup should succeed"),
            service_addr
        );
        assert!(
            client
                .unregister_tcp(service_addr, service, "owner")
                .await
                .expect("unregister should succeed")
        );
        assert!(matches!(
            client.lookup_tcp_addr(service).await,
            Err(BindError::NotFound { .. })
        ));

        task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_client_can_register_lookup_and_unregister_udp() {
        let state = RpcbindState::default();
        let mut server = ServerBuilder::new()
            .with_bind_addr(loopback())
            .build_async();
        server
            .register(
                Program {
                    number: RPCBIND_PROGRAM.program,
                    version: RPCBIND_PROGRAM.version,
                },
                AsyncRpcbindDispatch {
                    state: state.clone(),
                },
            )
            .expect("registration should succeed");
        let transport = TokioAsyncServerTransport::bind(server)
            .await
            .expect("rpcbind should bind");
        let addr = transport.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = transport.serve().await;
        });

        let client = AsyncRpcbindClient::connect(addr)
            .await
            .expect("connect should succeed");
        let service = ProgramVersion {
            program: 200_011,
            version: 3,
        };
        let service_addr: SocketAddr = "127.0.0.1:6060".parse().expect("valid addr");

        assert!(
            client
                .register_udp(service_addr, service, "owner")
                .await
                .expect("register should succeed")
        );
        assert_eq!(
            client
                .lookup_udp_addr(service)
                .await
                .expect("lookup should succeed"),
            service_addr
        );
        assert!(
            client
                .unregister_udp(service_addr, service, "owner")
                .await
                .expect("unregister should succeed")
        );
        assert!(matches!(
            client.lookup_udp_addr(service).await,
            Err(BindError::NotFound { .. })
        ));

        task.abort();
    }

    #[test]
    fn sync_client_can_register_lookup_and_unregister() {
        let state = RpcbindState::default();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let server_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("runtime should build");
            runtime.block_on(async move {
                let mut server = ServerBuilder::new()
                    .with_bind_addr(loopback())
                    .build_async();
                server
                    .register(
                        Program {
                            number: RPCBIND_PROGRAM.program,
                            version: RPCBIND_PROGRAM.version,
                        },
                        AsyncRpcbindDispatch { state },
                    )
                    .expect("registration should succeed");
                let transport = TokioAsyncServerTransport::bind(server)
                    .await
                    .expect("rpcbind should bind");
                let addr = transport.local_addr().expect("local addr");
                tx.send(addr).expect("send should succeed");
                transport
                    .accept_once()
                    .await
                    .expect("accept should succeed");
            });
        });
        let addr = rx.recv().expect("addr receive should succeed");

        let client = RpcbindClient::connect(addr).expect("connect should succeed");
        let service = ProgramVersion {
            program: 200_002,
            version: 8,
        };
        let service_addr: SocketAddr = "127.0.0.1:5050".parse().expect("valid addr");

        assert!(
            client
                .register_tcp(service_addr, service, "owner")
                .expect("register should succeed")
        );
        assert_eq!(
            client
                .lookup_tcp_addr(service)
                .expect("lookup should succeed"),
            service_addr
        );
        assert!(
            client
                .unregister_tcp(service_addr, service, "owner")
                .expect("unregister should succeed")
        );
        assert!(matches!(
            client.lookup_tcp_addr(service),
            Err(BindError::NotFound { .. })
        ));

        drop(client);
        server_thread.join().expect("server thread should join");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_server_transport_publish_and_unpublish_work() {
        let state = RpcbindState::default();
        let mut rpcbind = ServerBuilder::new()
            .with_bind_addr(loopback())
            .build_async();
        rpcbind
            .register(
                Program {
                    number: RPCBIND_PROGRAM.program,
                    version: RPCBIND_PROGRAM.version,
                },
                AsyncRpcbindDispatch {
                    state: state.clone(),
                },
            )
            .expect("registration should succeed");
        let rpcbind_transport = TokioAsyncServerTransport::bind(rpcbind)
            .await
            .expect("rpcbind should bind");
        let rpcbind_addr = rpcbind_transport.local_addr().expect("local addr");
        let rpcbind_task = tokio::spawn(async move {
            let _ = rpcbind_transport.serve().await;
        });

        let mut server = ServerBuilder::new()
            .with_bind_addr(loopback())
            .with_auto_publish(true)
            .with_service_name("time-service")
            .build_async();
        struct Noop;
        #[async_trait]
        impl AsyncDispatch for Noop {
            async fn dispatch(
                &self,
                _request: RequestContext,
            ) -> Result<ResponsePayload, DispatchError> {
                Ok(ResponsePayload::success(bytes::Bytes::new()))
            }
        }
        let published = Program {
            number: 300_001,
            version: 1,
        };
        server
            .register(published, Noop)
            .expect("register should succeed");
        let transport = TokioAsyncServerTransport::bind(server)
            .await
            .expect("service transport should bind");

        assert!(
            transport
                .maybe_publish_rpcbind(rpcbind_addr)
                .await
                .expect("publish should succeed")
        );

        let resolver = AsyncRpcbindClient::connect(rpcbind_addr)
            .await
            .expect("connect should succeed");
        let resolved = resolver
            .lookup_tcp_addr(ProgramVersion {
                program: published.number,
                version: published.version,
            })
            .await
            .expect("lookup should succeed");
        assert_eq!(resolved, transport.local_addr().expect("local addr"));

        transport
            .unpublish_rpcbind(rpcbind_addr)
            .await
            .expect("unpublish should succeed");
        assert!(matches!(
            resolver
                .lookup_tcp_addr(ProgramVersion {
                    program: published.number,
                    version: published.version,
                })
                .await,
            Err(BindError::NotFound { .. })
        ));

        rpcbind_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_udp_server_transport_publish_and_unpublish_work() {
        let state = RpcbindState::default();
        let mut rpcbind = ServerBuilder::new()
            .with_bind_addr(loopback())
            .build_async();
        rpcbind
            .register(
                Program {
                    number: RPCBIND_PROGRAM.program,
                    version: RPCBIND_PROGRAM.version,
                },
                AsyncRpcbindDispatch {
                    state: state.clone(),
                },
            )
            .expect("registration should succeed");
        let rpcbind_transport = TokioAsyncServerTransport::bind(rpcbind)
            .await
            .expect("rpcbind should bind");
        let rpcbind_addr = rpcbind_transport.local_addr().expect("local addr");
        let rpcbind_task = tokio::spawn(async move {
            let _ = rpcbind_transport.serve().await;
        });

        let mut server = ServerBuilder::new()
            .with_bind_addr(loopback())
            .with_auto_publish(true)
            .with_service_name("udp-time-service")
            .build_async();
        struct Noop;
        #[async_trait]
        impl AsyncDispatch for Noop {
            async fn dispatch(
                &self,
                _request: RequestContext,
            ) -> Result<ResponsePayload, DispatchError> {
                Ok(ResponsePayload::success(bytes::Bytes::new()))
            }
        }
        let published = Program {
            number: 300_011,
            version: 1,
        };
        server
            .register(published, Noop)
            .expect("register should succeed");
        let transport = TokioAsyncUdpServerTransport::bind(server)
            .await
            .expect("service transport should bind");

        assert!(
            transport
                .maybe_publish_rpcbind(rpcbind_addr)
                .await
                .expect("publish should succeed")
        );

        let resolver = AsyncRpcbindClient::connect(rpcbind_addr)
            .await
            .expect("connect should succeed");
        let resolved = resolver
            .lookup_udp_addr(ProgramVersion {
                program: published.number,
                version: published.version,
            })
            .await
            .expect("lookup should succeed");
        assert_eq!(resolved, transport.local_addr().expect("local addr"));

        transport
            .unpublish_rpcbind(rpcbind_addr)
            .await
            .expect("unpublish should succeed");
        assert!(matches!(
            resolver
                .lookup_udp_addr(ProgramVersion {
                    program: published.number,
                    version: published.version,
                })
                .await,
            Err(BindError::NotFound { .. })
        ));

        rpcbind_task.abort();
    }
}
