//! Server builder and registration APIs for ONC RPC.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Program {
    pub number: u32,
    pub version: u32,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub auto_publish: bool,
    pub service_name: Option<String>,
    pub selector_threads: usize,
    pub worker_threads: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            auto_publish: false,
            service_name: None,
            selector_threads: 1,
            worker_threads: 1,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("service registration is not implemented yet")]
    RegistrationNotImplemented,
}

pub trait Dispatch: Send + Sync + 'static {}

pub struct ServerBuilder {
    config: ServerConfig,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self {
            config: ServerConfig::default(),
        }
    }

    pub fn with_bind_addr(mut self, bind_addr: SocketAddr) -> Self {
        self.config.bind_addr = bind_addr;
        self
    }

    pub fn with_auto_publish(mut self, enabled: bool) -> Self {
        self.config.auto_publish = enabled;
        self
    }

    pub fn with_service_name(mut self, service_name: impl Into<String>) -> Self {
        self.config.service_name = Some(service_name.into());
        self
    }

    pub fn with_selector_threads(mut self, count: usize) -> Self {
        self.config.selector_threads = count;
        self
    }

    pub fn with_worker_threads(mut self, count: usize) -> Self {
        self.config.worker_threads = count;
        self
    }

    pub fn build(self) -> Server {
        Server {
            config: self.config,
        }
    }
}

pub struct Server {
    config: ServerConfig,
}

impl Server {
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn register<D: Dispatch>(
        &mut self,
        _program: Program,
        _dispatch: D,
    ) -> Result<(), ServerError> {
        Err(ServerError::RegistrationNotImplemented)
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
