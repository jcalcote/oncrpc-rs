//! Minimal server-side ONC RPC contracts for generated dispatch stubs.

mod transport;

pub use async_trait::async_trait;
use bytes::Bytes;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, MessageBody, OpaqueAuth, Procedure, ProgramVersion, ReplyBody,
    RpcMessage, Xid,
};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use thiserror::Error;

pub use transport::{ServerTransportError, TokioAsyncServerTransport, TokioServerTransport};

type DispatchResolution = (Xid, RequestContext, Option<Arc<dyn Dispatch>>);
type AsyncDispatchResolution = (Xid, RequestContext, Option<Arc<dyn AsyncDispatch>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Program {
    pub number: u32,
    pub version: u32,
}

impl From<ProgramVersion> for Program {
    fn from(value: ProgramVersion) -> Self {
        Self {
            number: value.program,
            version: value.version,
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestContext {
    pub xid: Xid,
    pub program: ProgramVersion,
    pub procedure: Procedure,
    pub credentials: OpaqueAuth,
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsePayload {
    pub verifier: OpaqueAuth,
    pub payload: Bytes,
}

impl ResponsePayload {
    pub fn success(payload: Bytes) -> Self {
        Self {
            verifier: OpaqueAuth::none(),
            payload,
        }
    }
}

impl RequestContext {
    pub fn decode_credentials(&self) -> Result<onc_rpc_auth::AuthFlavor, onc_rpc_auth::AuthError> {
        onc_rpc_auth::decode_auth(&self.credentials)
    }

    pub fn decode_verifier(&self) -> Result<onc_rpc_auth::AuthFlavor, onc_rpc_auth::AuthError> {
        onc_rpc_auth::decode_auth(&self.verifier)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DispatchError {
    #[error("procedure is unavailable")]
    ProcedureUnavailable,
    #[error("request arguments are invalid")]
    GarbageArgs,
    #[error("system error while servicing request")]
    SystemError,
}

pub trait Dispatch: Send + Sync + 'static {
    fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError>;
}

#[async_trait]
pub trait AsyncDispatch: Send + Sync + 'static {
    async fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError>;
}

pub trait ServerTransportIntrospection {
    fn config(&self) -> &ServerConfig;
    fn registered_programs(&self) -> Vec<Program>;
}

pub trait AsyncServerTransportIntrospection {
    fn config(&self) -> &ServerConfig;
    fn registered_programs(&self) -> Vec<Program>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ServerError {
    #[error("service already registered for program {0:?}")]
    DuplicateRegistration(Program),
    #[error("received an rpc reply message where a call was expected")]
    UnexpectedReplyMessage,
}

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
            dispatchers: HashMap::new(),
        }
    }

    pub fn build_async(self) -> AsyncServer {
        AsyncServer {
            config: self.config,
            dispatchers: HashMap::new(),
        }
    }
}

pub struct Server {
    config: ServerConfig,
    dispatchers: HashMap<Program, Arc<dyn Dispatch>>,
}

pub struct AsyncServer {
    config: ServerConfig,
    dispatchers: HashMap<Program, Arc<dyn AsyncDispatch>>,
}

impl Server {
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn registered_programs(&self) -> Vec<Program> {
        let mut programs = self.dispatchers.keys().copied().collect::<Vec<_>>();
        programs.sort_by_key(|program| (program.number, program.version));
        programs
    }

    pub fn register<D: Dispatch>(
        &mut self,
        program: Program,
        dispatch: D,
    ) -> Result<(), ServerError> {
        match self.dispatchers.entry(program) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(ServerError::DuplicateRegistration(program))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Arc::new(dispatch));
                Ok(())
            }
        }
    }

    pub fn handle_message(&self, message: RpcMessage) -> Result<RpcMessage, ServerError> {
        let (xid, request, dispatcher) = self.resolve_dispatch(message)?;
        let body = match dispatcher {
            Some(dispatch) => match dispatch.dispatch(request) {
                Ok(response) => success_reply(response),
                Err(DispatchError::ProcedureUnavailable) => procedure_unavailable_reply(),
                Err(DispatchError::GarbageArgs) => garbage_args_reply(),
                Err(DispatchError::SystemError) => system_error_reply(),
            },
            None => program_unavailable_reply(),
        };

        Ok(RpcMessage {
            xid,
            body: MessageBody::Reply(body),
        })
    }

    fn resolve_dispatch(&self, message: RpcMessage) -> Result<DispatchResolution, ServerError> {
        let xid = message.xid;

        let MessageBody::Call(call) = message.body else {
            return Err(ServerError::UnexpectedReplyMessage);
        };

        let program = ProgramVersion {
            program: call.program.program,
            version: call.program.version,
        };
        let dispatch_key = Program::from(program);
        let request = RequestContext {
            xid,
            program,
            procedure: call.procedure,
            credentials: call.credentials,
            verifier: call.verifier,
            payload: call.payload,
        };

        Ok((xid, request, self.dispatchers.get(&dispatch_key).cloned()))
    }
}

impl AsyncServer {
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn registered_programs(&self) -> Vec<Program> {
        let mut programs = self.dispatchers.keys().copied().collect::<Vec<_>>();
        programs.sort_by_key(|program| (program.number, program.version));
        programs
    }

    pub fn register<D: AsyncDispatch>(
        &mut self,
        program: Program,
        dispatch: D,
    ) -> Result<(), ServerError> {
        match self.dispatchers.entry(program) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(ServerError::DuplicateRegistration(program))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Arc::new(dispatch));
                Ok(())
            }
        }
    }

    pub async fn handle_message(&self, message: RpcMessage) -> Result<RpcMessage, ServerError> {
        let (xid, request, dispatcher) = self.resolve_dispatch(message)?;
        let body = match dispatcher {
            Some(dispatch) => match dispatch.dispatch(request).await {
                Ok(response) => success_reply(response),
                Err(DispatchError::ProcedureUnavailable) => procedure_unavailable_reply(),
                Err(DispatchError::GarbageArgs) => garbage_args_reply(),
                Err(DispatchError::SystemError) => system_error_reply(),
            },
            None => program_unavailable_reply(),
        };

        Ok(RpcMessage {
            xid,
            body: MessageBody::Reply(body),
        })
    }

    fn resolve_dispatch(
        &self,
        message: RpcMessage,
    ) -> Result<AsyncDispatchResolution, ServerError> {
        let xid = message.xid;

        let MessageBody::Call(call) = message.body else {
            return Err(ServerError::UnexpectedReplyMessage);
        };

        let program = ProgramVersion {
            program: call.program.program,
            version: call.program.version,
        };
        let dispatch_key = Program::from(program);
        let request = RequestContext {
            xid,
            program,
            procedure: call.procedure,
            credentials: call.credentials,
            verifier: call.verifier,
            payload: call.payload,
        };

        Ok((xid, request, self.dispatchers.get(&dispatch_key).cloned()))
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn success_reply(response: ResponsePayload) -> ReplyBody {
    ReplyBody::Accepted(AcceptedReply {
        verifier: response.verifier,
        status: AcceptedStatus::Success(response.payload),
    })
}

fn procedure_unavailable_reply() -> ReplyBody {
    ReplyBody::Accepted(AcceptedReply {
        verifier: OpaqueAuth::none(),
        status: AcceptedStatus::ProcedureUnavailable,
    })
}

fn garbage_args_reply() -> ReplyBody {
    ReplyBody::Accepted(AcceptedReply {
        verifier: OpaqueAuth::none(),
        status: AcceptedStatus::GarbageArgs,
    })
}

fn system_error_reply() -> ReplyBody {
    ReplyBody::Accepted(AcceptedReply {
        verifier: OpaqueAuth::none(),
        status: AcceptedStatus::SystemError,
    })
}

fn program_unavailable_reply() -> ReplyBody {
    ReplyBody::Accepted(AcceptedReply {
        verifier: OpaqueAuth::none(),
        status: AcceptedStatus::ProgramUnavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoDispatch;

    impl Dispatch for EchoDispatch {
        fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
            if request.procedure == Procedure(1) {
                Ok(ResponsePayload::success(request.payload))
            } else {
                Err(DispatchError::ProcedureUnavailable)
            }
        }
    }

    struct AsyncEchoDispatch;

    #[async_trait]
    impl AsyncDispatch for AsyncEchoDispatch {
        async fn dispatch(
            &self,
            request: RequestContext,
        ) -> Result<ResponsePayload, DispatchError> {
            if request.procedure == Procedure(1) {
                Ok(ResponsePayload::success(request.payload))
            } else {
                Err(DispatchError::ProcedureUnavailable)
            }
        }
    }

    fn call_message(
        program: ProgramVersion,
        procedure: Procedure,
        payload: &'static [u8],
    ) -> RpcMessage {
        RpcMessage {
            xid: Xid(77),
            body: MessageBody::Call(onc_rpc_wire::CallBody::new(
                program,
                procedure,
                OpaqueAuth::none(),
                OpaqueAuth::none(),
                Bytes::from_static(payload),
            )),
        }
    }

    #[test]
    fn server_dispatches_registered_program() {
        let mut server = ServerBuilder::new().build();
        server
            .register(
                Program {
                    number: 100_003,
                    version: 3,
                },
                EchoDispatch,
            )
            .expect("registration should succeed");

        let reply = server
            .handle_message(call_message(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(1),
                b"echo",
            ))
            .expect("dispatch should succeed");

        assert_eq!(reply.xid, Xid(77));
        match reply.body {
            MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                status: AcceptedStatus::Success(payload),
                ..
            })) => assert_eq!(payload, Bytes::from_static(b"echo")),
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[test]
    fn server_returns_program_unavailable_for_missing_registration() {
        let server = ServerBuilder::new().build();
        let reply = server
            .handle_message(call_message(
                ProgramVersion {
                    program: 999,
                    version: 1,
                },
                Procedure(1),
                b"missing",
            ))
            .expect("server should synthesize rpc reply");

        match reply.body {
            MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                status: AcceptedStatus::ProgramUnavailable,
                ..
            })) => {}
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[test]
    fn server_returns_procedure_unavailable_from_dispatch() {
        let mut server = ServerBuilder::new().build();
        server
            .register(
                Program {
                    number: 100_003,
                    version: 3,
                },
                EchoDispatch,
            )
            .expect("registration should succeed");

        let reply = server
            .handle_message(call_message(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(9),
                b"missing-proc",
            ))
            .expect("server should synthesize rpc reply");

        match reply.body {
            MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                status: AcceptedStatus::ProcedureUnavailable,
                ..
            })) => {}
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[test]
    fn server_rejects_duplicate_registration() {
        let mut server = ServerBuilder::new().build();
        let program = Program {
            number: 100_003,
            version: 3,
        };
        server
            .register(program, EchoDispatch)
            .expect("initial registration should succeed");

        let error = server
            .register(program, EchoDispatch)
            .expect_err("duplicate registration must fail");

        assert_eq!(error, ServerError::DuplicateRegistration(program));
    }

    #[tokio::test]
    async fn async_server_dispatches_registered_program() {
        let mut server = ServerBuilder::new().build_async();
        server
            .register(
                Program {
                    number: 100_003,
                    version: 3,
                },
                AsyncEchoDispatch,
            )
            .expect("registration should succeed");

        let reply = server
            .handle_message(call_message(
                ProgramVersion {
                    program: 100_003,
                    version: 3,
                },
                Procedure(1),
                b"echo",
            ))
            .await
            .expect("dispatch should succeed");

        assert_eq!(reply.xid, Xid(77));
        match reply.body {
            MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                status: AcceptedStatus::Success(payload),
                ..
            })) => assert_eq!(payload, Bytes::from_static(b"echo")),
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[tokio::test]
    async fn async_server_returns_program_unavailable_for_missing_registration() {
        let server = ServerBuilder::new().build_async();
        let reply = server
            .handle_message(call_message(
                ProgramVersion {
                    program: 999,
                    version: 1,
                },
                Procedure(1),
                b"missing",
            ))
            .await
            .expect("server should synthesize rpc reply");

        match reply.body {
            MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                status: AcceptedStatus::ProgramUnavailable,
                ..
            })) => {}
            other => panic!("unexpected reply {other:?}"),
        }
    }
}
