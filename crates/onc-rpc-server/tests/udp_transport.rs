use onc_rpc_auth::{AuthFlavor, AuthSys};
use onc_rpc_runtime::{
    AsyncClient, Client, ClientConfig, Procedure, ProgramVersion, TokioAsyncUdpClientTransport,
    TokioUdpClientTransport,
};
use onc_rpc_server::{
    AsyncDispatch, Dispatch, DispatchError, Program, RequestContext, ResponsePayload,
    ServerBuilder, TokioAsyncUdpServerTransport, TokioUdpServerTransport, async_trait,
};
use onc_rpc_xdr::XdrEncode;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;

struct SyncEchoDispatch;

impl Dispatch for SyncEchoDispatch {
    fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        Ok(ResponsePayload::success(request.payload))
    }
}

struct AsyncEchoDispatch;

#[async_trait]
impl AsyncDispatch for AsyncEchoDispatch {
    async fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        Ok(ResponsePayload::success(request.payload))
    }
}

struct AsyncConcurrentDispatch {
    current: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
}

#[async_trait]
impl AsyncDispatch for AsyncConcurrentDispatch {
    async fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        let in_flight = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(in_flight, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(ResponsePayload::success(request.payload))
    }
}

struct AsyncAuthInspectDispatch;

#[async_trait]
impl AsyncDispatch for AsyncAuthInspectDispatch {
    async fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        match request
            .decode_credentials()
            .map_err(|_| DispatchError::GarbageArgs)?
        {
            AuthFlavor::Sys(auth) => Ok(ResponsePayload::success(
                auth.uid
                    .to_xdr_bytes()
                    .map_err(|_| DispatchError::SystemError)?,
            )),
            AuthFlavor::None => Err(DispatchError::ProcedureUnavailable),
        }
    }
}

fn loopback_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

#[tokio::test]
async fn async_udp_server_transport_dispatches_over_real_tokio_io() {
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncEchoDispatch,
        )
        .expect("registration should succeed");

    let transport = TokioAsyncUdpServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let client_transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, client_transport);

    let reply: u32 = client
        .call_typed(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &99_u32,
        )
        .await
        .expect("call should succeed");

    assert_eq!(reply, 99);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[tokio::test]
async fn async_udp_server_transport_decodes_auth_sys_credentials() {
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncAuthInspectDispatch,
        )
        .expect("registration should succeed");

    let transport = TokioAsyncUdpServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let auth_sys = AuthSys {
        stamp: 9,
        machine_name: "udp-auth".into(),
        uid: 2000,
        gid: 200,
        gids: vec![201],
    };
    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2))
        .with_auth_sys(&auth_sys)
        .expect("auth sys should encode");
    let client_transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, client_transport);

    let reply: u32 = client
        .call_typed(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &99_u32,
        )
        .await
        .expect("call should succeed");

    assert_eq!(reply, 2000);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[tokio::test]
async fn async_udp_server_transport_handles_concurrent_requests() {
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .with_worker_threads(2)
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncEchoDispatch,
        )
        .expect("registration should succeed");

    let transport = TokioAsyncUdpServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.serve().await });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let client_transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, client_transport);

    let first = client.call_typed::<u32, u32>(
        ProgramVersion {
            program: 100_003,
            version: 3,
        },
        Procedure(1),
        &1_u32,
    );
    let second = client.call_typed::<u32, u32>(
        ProgramVersion {
            program: 100_003,
            version: 3,
        },
        Procedure(1),
        &2_u32,
    );

    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.expect("first call should succeed"), 1);
    assert_eq!(second.expect("second call should succeed"), 2);

    serve.abort();
}

#[tokio::test]
async fn async_udp_server_transport_uses_multiple_workers_when_available() {
    let current = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .with_worker_threads(4)
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncConcurrentDispatch {
                current: current.clone(),
                max_seen: max_seen.clone(),
            },
        )
        .expect("registration should succeed");

    let transport = TokioAsyncUdpServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.serve().await });

    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let client_transport = TokioAsyncUdpClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = Arc::new(AsyncClient::new(config, client_transport));
    let mut tasks = JoinSet::new();

    for value in 0..8_u32 {
        let client = client.clone();
        tasks.spawn(async move {
            client
                .call_typed::<u32, u32>(
                    ProgramVersion {
                        program: 100_003,
                        version: 3,
                    },
                    Procedure(1),
                    &value,
                )
                .await
        });
    }

    while let Some(result) = tasks.join_next().await {
        result
            .expect("task should join")
            .expect("call should succeed");
    }

    assert!(max_seen.load(Ordering::SeqCst) > 1);
    serve.abort();
}

#[test]
fn sync_udp_server_transport_dispatches_over_real_tokio_io() {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let server_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        runtime.block_on(async move {
            let mut server = ServerBuilder::new().with_bind_addr(loopback_addr()).build();
            server
                .register(
                    Program {
                        number: 100_003,
                        version: 3,
                    },
                    SyncEchoDispatch,
                )
                .expect("registration should succeed");
            let transport = TokioUdpServerTransport::bind(server)
                .await
                .expect("server should bind");
            let addr = transport.local_addr().expect("local addr");
            tx.send(addr).expect("addr send should succeed");
            transport
                .accept_once()
                .await
                .expect("server should complete");
        });
    });

    let addr = rx.recv().expect("addr receive should succeed");
    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(2));
    let client_transport = TokioUdpClientTransport::connect(&config).expect("client connect");
    let client = Client::new(config, client_transport);

    let reply: u32 = client
        .call_typed(
            ProgramVersion {
                program: 100_003,
                version: 3,
            },
            Procedure(1),
            &123_u32,
        )
        .expect("call should succeed");

    assert_eq!(reply, 123);
    server_thread.join().expect("server thread should join");
}
