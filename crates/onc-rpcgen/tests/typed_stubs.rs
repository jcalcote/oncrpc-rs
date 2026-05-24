use bytes::Bytes;
use onc_rpc_runtime::async_trait as runtime_async_trait;
use onc_rpc_runtime::{
    AcceptedReply, AcceptedStatus, AsyncClient, AsyncClientTransport, CallOptions, CallTimeout,
    Client, ClientConfig, ClientTransport, MessageBody, OpaqueAuth, Procedure, ProgramVersion,
    ReplyBody, RpcMessage, RuntimeError, TokioAsyncClientTransport, Xid,
};
use onc_rpc_server::{
    AsyncDispatch, Dispatch, DispatchError, Program, RequestContext, ServerBuilder,
    TokioAsyncServerTransport, async_trait as server_async_trait,
};
use onc_rpc_xdr::{XdrDecode, XdrEncode};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod common_types {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/common_types.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod nfs_support {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/nfs_support.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod transfer_types {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/transfer_types.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod blob_service_basic {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/blob_service_basic.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod blob_service_basic_stubs {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/blob_service_basic.generated.stubs.rs.txt"
    ));
}

struct CaptureTransport {
    seen: Arc<Mutex<Option<RpcMessage>>>,
    seen_options: Arc<Mutex<Option<CallOptions>>>,
    reply_payload: Bytes,
}

impl ClientTransport for CaptureTransport {
    fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        *self.seen.lock().expect("mutex poisoned") = Some(request.clone());
        Ok(RpcMessage {
            xid: request.xid,
            body: MessageBody::Reply(ReplyBody::Accepted(AcceptedReply {
                verifier: OpaqueAuth::none(),
                status: AcceptedStatus::Success(self.reply_payload.clone()),
            })),
        })
    }

    fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        *self.seen_options.lock().expect("mutex poisoned") = Some(options.clone());
        ClientTransport::call(self, request)
    }
}

#[runtime_async_trait]
impl AsyncClientTransport for CaptureTransport {
    async fn call(&self, request: RpcMessage) -> Result<RpcMessage, RuntimeError> {
        ClientTransport::call(self, request)
    }

    async fn call_with_options(
        &self,
        request: RpcMessage,
        options: &CallOptions,
    ) -> Result<RpcMessage, RuntimeError> {
        ClientTransport::call_with_options(self, request, options)
    }
}

fn client_config() -> ClientConfig {
    ClientConfig::new(
        "127.0.0.1:2049"
            .parse::<SocketAddr>()
            .expect("socket addr should parse"),
    )
}

fn sample_request() -> blob_service_basic::copy_request_t {
    blob_service_basic::copy_request_t {
        job_id: 7,
        job_update: 3,
        source_instance: transfer_types::instance_info_t {
            source_handle: Bytes::from_static(b"source-handle"),
        },
    }
}

fn sample_reply() -> transfer_types::job_result_t {
    transfer_types::job_result_t { status: 42 }
}

#[test]
fn generated_typed_client_marshals_request_and_reply_payloads() {
    let seen = Arc::new(Mutex::new(None));
    let expected_reply = sample_reply();
    let reply_payload = expected_reply
        .to_xdr_bytes()
        .expect("reply payload should encode");
    let transport = CaptureTransport {
        seen: seen.clone(),
        seen_options: Arc::new(Mutex::new(None)),
        reply_payload,
    };
    let client = Client::new(client_config(), transport);
    let stub =
        blob_service_basic_stubs::blob_service::blob_service_v1::client::BLOB_SERVICE_V1Client::new(
            client,
        );
    let expected_request = sample_request();

    let reply = stub
        .blob_copy(expected_request.clone())
        .expect("typed client call should succeed");

    assert_eq!(reply, expected_reply);

    let seen = seen
        .lock()
        .expect("mutex poisoned")
        .clone()
        .expect("request should be captured");
    let MessageBody::Call(call) = seen.body else {
        panic!("expected call message");
    };
    assert_eq!(
        call.program,
        ProgramVersion {
            program: 200001,
            version: 1,
        }
    );
    assert_eq!(call.procedure, Procedure(1));
    let decoded =
        blob_service_basic::copy_request_t::from_xdr_bytes(&call.payload).expect("decode request");
    assert_eq!(decoded, expected_request);
}

#[test]
fn generated_typed_client_with_options_forwards_call_options() {
    let seen = Arc::new(Mutex::new(None));
    let seen_options = Arc::new(Mutex::new(None));
    let expected_reply = sample_reply();
    let reply_payload = expected_reply
        .to_xdr_bytes()
        .expect("reply payload should encode");
    let transport = CaptureTransport {
        seen,
        seen_options: seen_options.clone(),
        reply_payload,
    };
    let client = Client::new(client_config(), transport);
    let stub =
        blob_service_basic_stubs::blob_service::blob_service_v1::client::BLOB_SERVICE_V1Client::new(
            client,
        );

    let response = stub
        .blob_copy_with_options(
            sample_request(),
            &CallOptions::new().with_timeout(std::time::Duration::from_secs(30)),
        )
        .expect("typed client call should succeed");

    assert_eq!(response, expected_reply);
    assert_eq!(
        *seen_options.lock().expect("mutex poisoned"),
        Some(CallOptions {
            timeout: CallTimeout::Duration(std::time::Duration::from_secs(30)),
        })
    );
}

struct TypedService {
    seen: Arc<Mutex<Vec<blob_service_basic::copy_request_t>>>,
}

impl blob_service_basic_stubs::blob_service::blob_service_v1::server::BLOB_SERVICE_V1Service
    for TypedService
{
    fn blob_null(&self) -> Result<(), DispatchError> {
        Ok(())
    }

    fn blob_copy(
        &self,
        argument: blob_service_basic::copy_request_t,
    ) -> Result<transfer_types::job_result_t, DispatchError> {
        self.seen.lock().expect("mutex poisoned").push(argument);
        Ok(sample_reply())
    }
}

#[test]
fn generated_typed_dispatch_unmarshals_request_and_marshals_reply_payloads() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatch =
        blob_service_basic_stubs::blob_service::blob_service_v1::server::BLOB_SERVICE_V1Dispatch::new(
            TypedService { seen: seen.clone() },
        );
    let expected_request = sample_request();
    let request_payload = expected_request
        .to_xdr_bytes()
        .expect("request payload should encode");

    let response = dispatch
        .dispatch(RequestContext {
            xid: Xid(5),
            program: ProgramVersion {
                program: 200001,
                version: 1,
            },
            procedure: Procedure(1),
            credentials: OpaqueAuth::none(),
            verifier: OpaqueAuth::none(),
            payload: request_payload,
        })
        .expect("dispatch should succeed");

    let decoded_reply =
        transfer_types::job_result_t::from_xdr_bytes(&response.payload).expect("decode reply");
    assert_eq!(decoded_reply, sample_reply());
    assert_eq!(
        seen.lock().expect("mutex poisoned").as_slice(),
        &[expected_request]
    );
}

#[tokio::test]
async fn generated_async_typed_client_marshals_request_and_reply_payloads() {
    let seen = Arc::new(Mutex::new(None));
    let expected_reply = sample_reply();
    let reply_payload = expected_reply
        .to_xdr_bytes()
        .expect("reply payload should encode");
    let transport = CaptureTransport {
        seen: seen.clone(),
        seen_options: Arc::new(Mutex::new(None)),
        reply_payload,
    };
    let client = AsyncClient::new(client_config(), transport);
    let stub = blob_service_basic_stubs::blob_service::blob_service_v1::async_client::BLOB_SERVICE_V1Client::new(
        client,
    );
    let expected_request = sample_request();

    let reply = stub
        .blob_copy(expected_request.clone())
        .await
        .expect("typed async client call should succeed");

    assert_eq!(reply, expected_reply);

    let seen = seen
        .lock()
        .expect("mutex poisoned")
        .clone()
        .expect("request should be captured");
    let MessageBody::Call(call) = seen.body else {
        panic!("expected call message");
    };
    let decoded =
        blob_service_basic::copy_request_t::from_xdr_bytes(&call.payload).expect("decode request");
    assert_eq!(decoded, expected_request);
}

#[tokio::test]
async fn generated_async_typed_client_with_options_forwards_call_options() {
    let seen = Arc::new(Mutex::new(None));
    let seen_options = Arc::new(Mutex::new(None));
    let expected_reply = sample_reply();
    let reply_payload = expected_reply
        .to_xdr_bytes()
        .expect("reply payload should encode");
    let transport = CaptureTransport {
        seen,
        seen_options: seen_options.clone(),
        reply_payload,
    };
    let client = AsyncClient::new(client_config(), transport);
    let stub = blob_service_basic_stubs::blob_service::blob_service_v1::async_client::BLOB_SERVICE_V1Client::new(
        client,
    );

    let response = stub
        .blob_copy_with_options(sample_request(), &CallOptions::new().without_timeout())
        .await
        .expect("typed async client call should succeed");

    assert_eq!(response, expected_reply);
    assert_eq!(
        *seen_options.lock().expect("mutex poisoned"),
        Some(CallOptions {
            timeout: CallTimeout::None,
        })
    );
}

struct AsyncTypedService {
    seen: Arc<Mutex<Vec<blob_service_basic::copy_request_t>>>,
}

#[server_async_trait]
impl blob_service_basic_stubs::blob_service::blob_service_v1::async_server::BLOB_SERVICE_V1Service
    for AsyncTypedService
{
    async fn blob_null(&self) -> Result<(), DispatchError> {
        Ok(())
    }

    async fn blob_copy(
        &self,
        argument: blob_service_basic::copy_request_t,
    ) -> Result<transfer_types::job_result_t, DispatchError> {
        self.seen.lock().expect("mutex poisoned").push(argument);
        Ok(sample_reply())
    }
}

#[tokio::test]
async fn generated_async_typed_dispatch_unmarshals_request_and_marshals_reply_payloads() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatch = blob_service_basic_stubs::blob_service::blob_service_v1::async_server::BLOB_SERVICE_V1Dispatch::new(
        AsyncTypedService { seen: seen.clone() },
    );
    let expected_request = sample_request();
    let request_payload = expected_request
        .to_xdr_bytes()
        .expect("request payload should encode");

    let response = dispatch
        .dispatch(RequestContext {
            xid: Xid(6),
            program: ProgramVersion {
                program: 200001,
                version: 1,
            },
            procedure: Procedure(1),
            credentials: OpaqueAuth::none(),
            verifier: OpaqueAuth::none(),
            payload: request_payload,
        })
        .await
        .expect("async dispatch should succeed");

    let decoded_reply =
        transfer_types::job_result_t::from_xdr_bytes(&response.payload).expect("decode reply");
    assert_eq!(decoded_reply, sample_reply());
    assert_eq!(
        seen.lock().expect("mutex poisoned").as_slice(),
        &[expected_request]
    );
}

#[tokio::test]
async fn generated_async_stubs_round_trip_over_real_tokio_transport() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut server = ServerBuilder::new()
        .with_bind_addr(
            "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("socket addr should parse"),
        )
        .build_async();
    server
        .register(
            Program {
                number: 200001,
                version: 1,
            },
            blob_service_basic_stubs::blob_service::blob_service_v1::async_server::BLOB_SERVICE_V1Dispatch::new(
                AsyncTypedService { seen: seen.clone() },
            ),
        )
        .expect("registration should succeed");

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let config = ClientConfig::new(addr).with_connect_timeout(std::time::Duration::from_secs(5));
    let client_transport = TokioAsyncClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let stub = Arc::new(
        blob_service_basic_stubs::blob_service::blob_service_v1::async_client::BLOB_SERVICE_V1Client::new(
            AsyncClient::new(config, client_transport),
        ),
    );

    let mut tasks = JoinSet::new();
    for job_id in 0..8_u64 {
        let stub = stub.clone();
        tasks.spawn(async move {
            let mut request = sample_request();
            request.job_id = job_id;
            let reply = stub.blob_copy(request.clone()).await;
            (request, reply)
        });
    }

    let mut observed_ids = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let (request, reply) = result.expect("task should join");
        assert_eq!(reply.expect("stub call should succeed"), sample_reply());
        observed_ids.push(request.job_id);
    }
    observed_ids.sort_unstable();

    let mut seen_ids = seen
        .lock()
        .expect("mutex poisoned")
        .iter()
        .map(|request| request.job_id)
        .collect::<Vec<_>>();
    seen_ids.sort_unstable();
    assert_eq!(seen_ids, observed_ids);

    drop(stub);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}
