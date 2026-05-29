use onc_rpc_auth::{AuthFlavor, AuthSys};
use onc_rpc_runtime::{
    AsyncClient, Client, ClientConfig, Procedure, ProgramVersion, TokioAsyncClientTransport,
    TokioClientTransport,
};
use onc_rpc_server::{
    AsyncDispatch, Dispatch, DispatchError, Program, RequestContext, ResponsePayload,
    ServerBuilder, TokioAsyncServerTransport, TokioServerTransport, async_trait,
};
use onc_rpc_xdr::{XdrDecode, XdrEncode};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinSet;

struct SyncEchoDispatch;

impl Dispatch for SyncEchoDispatch {
    fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        Ok(ResponsePayload::success(request.payload))
    }
}

struct SyncPeerInspectDispatch {
    seen_peer: Arc<Mutex<Option<SocketAddr>>>,
}

impl Dispatch for SyncPeerInspectDispatch {
    fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        *self.seen_peer.lock().expect("mutex poisoned") = request.peer_addr;
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

struct AsyncReorderingDispatch;

#[async_trait]
impl AsyncDispatch for AsyncReorderingDispatch {
    async fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        let value =
            u32::from_xdr_bytes(&request.payload).map_err(|_| DispatchError::SystemError)?;
        if value == 1 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Ok(ResponsePayload::success(
            value
                .to_xdr_bytes()
                .map_err(|_| DispatchError::SystemError)?,
        ))
    }
}

struct AsyncConcurrentDispatch {
    current: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
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

struct AsyncPeerInspectDispatch {
    seen_peer: Arc<Mutex<Option<SocketAddr>>>,
}

#[async_trait]
impl AsyncDispatch for AsyncPeerInspectDispatch {
    async fn dispatch(&self, request: RequestContext) -> Result<ResponsePayload, DispatchError> {
        *self.seen_peer.lock().expect("mutex poisoned") = request.peer_addr;
        Ok(ResponsePayload::success(request.payload))
    }
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

#[tokio::test]
async fn async_server_transport_exposes_tcp_peer_addr_to_dispatch() {
    let seen_peer = Arc::new(Mutex::new(None));
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncPeerInspectDispatch {
                seen_peer: seen_peer.clone(),
            },
        )
        .expect("registration should succeed");

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let config = ClientConfig::new(addr).with_connect_timeout(Duration::from_secs(5));
    let client_transport = TokioAsyncClientTransport::connect(&config)
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
    let peer = seen_peer
        .lock()
        .expect("mutex poisoned")
        .expect("peer addr should be captured");
    assert!(peer.ip().is_loopback());
    assert_ne!(peer.port(), 0);

    drop(client);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

fn loopback_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

#[tokio::test]
async fn async_server_transport_dispatches_over_real_tokio_io() {
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

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let client_transport = TokioAsyncClientTransport::connect(&config)
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
    drop(client);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[tokio::test]
async fn async_server_transport_decodes_auth_sys_credentials() {
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

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let auth_sys = AuthSys {
        stamp: 7,
        machine_name: "server-auth".into(),
        uid: 1000,
        gid: 100,
        gids: vec![101],
    };
    let config = ClientConfig::new(addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_auth_sys(&auth_sys)
        .expect("auth sys should encode");
    let client_transport = TokioAsyncClientTransport::connect(&config)
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

    assert_eq!(reply, 1000);
    drop(client);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[tokio::test]
async fn async_server_transport_handles_concurrent_requests_on_one_connection() {
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncReorderingDispatch,
        )
        .expect("registration should succeed");

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let client_transport = TokioAsyncClientTransport::connect(&config)
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

    drop(client);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[tokio::test]
async fn async_server_transport_honors_worker_thread_limit() {
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .with_selector_threads(2)
        .with_worker_threads(1)
        .build_async();
    server
        .register(
            Program {
                number: 100_003,
                version: 3,
            },
            AsyncReorderingDispatch,
        )
        .expect("registration should succeed");

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let config = ClientConfig::new(addr).with_connect_timeout(Duration::from_secs(5));
    let client_transport = TokioAsyncClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = AsyncClient::new(config, client_transport);

    let started = tokio::time::Instant::now();
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

    let (_, second_result) = tokio::join!(first, second);
    assert_eq!(second_result.expect("second call should succeed"), 2);
    assert!(
        started.elapsed() >= Duration::from_millis(45),
        "worker limit did not serialize request handling"
    );

    drop(client);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[tokio::test]
async fn async_server_transport_uses_multiple_workers_when_available() {
    let current = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let mut server = ServerBuilder::new()
        .with_bind_addr(loopback_addr())
        .with_selector_threads(2)
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

    let transport = TokioAsyncServerTransport::bind(server)
        .await
        .expect("server should bind");
    let addr = transport.local_addr().expect("local addr");
    let serve = tokio::spawn(async move { transport.accept_once().await });

    let config = ClientConfig::new(addr).with_connect_timeout(Duration::from_secs(5));
    let client_transport = TokioAsyncClientTransport::connect(&config)
        .await
        .expect("client should connect");
    let client = Arc::new(AsyncClient::new(config, client_transport));
    let mut tasks = JoinSet::new();

    for value in 0..12_u32 {
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

    let mut replies = Vec::new();
    while let Some(result) = tasks.join_next().await {
        replies.push(
            result
                .expect("task should join")
                .expect("call should succeed"),
        );
    }
    replies.sort_unstable();

    assert_eq!(replies, (0..12_u32).collect::<Vec<_>>());
    assert!(
        max_seen.load(Ordering::SeqCst) > 1,
        "worker pool never processed requests concurrently"
    );

    drop(client);
    serve
        .await
        .expect("server task should join")
        .expect("server transport should complete");
}

#[test]
fn sync_server_transport_dispatches_over_real_tokio_io() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let addr = runtime.block_on(async {
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

        let transport = TokioServerTransport::bind(server)
            .await
            .expect("server should bind");
        let addr = transport.local_addr().expect("local addr");
        tokio::spawn(async move {
            transport
                .accept_once()
                .await
                .expect("server transport should complete");
        });
        addr
    });

    let mut config = ClientConfig::new(addr);
    config.connect_timeout = Duration::from_secs(5);
    let transport = TokioClientTransport::connect(&config).expect("client should connect");
    let client = Client::new(config, transport);

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
}

#[test]
fn sync_server_transport_exposes_tcp_peer_addr_to_dispatch() {
    let seen_peer = Arc::new(Mutex::new(None));
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let addr = runtime.block_on(async {
        let mut server = ServerBuilder::new().with_bind_addr(loopback_addr()).build();
        server
            .register(
                Program {
                    number: 100_003,
                    version: 3,
                },
                SyncPeerInspectDispatch {
                    seen_peer: seen_peer.clone(),
                },
            )
            .expect("registration should succeed");

        let transport = TokioServerTransport::bind(server)
            .await
            .expect("server should bind");
        let addr = transport.local_addr().expect("local addr");
        tokio::spawn(async move {
            transport
                .accept_once()
                .await
                .expect("server transport should complete");
        });
        addr
    });

    let config = ClientConfig::new(addr).with_connect_timeout(Duration::from_secs(5));
    let transport = TokioClientTransport::connect(&config).expect("client should connect");
    let client = Client::new(config, transport);

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
    let peer = seen_peer
        .lock()
        .expect("mutex poisoned")
        .expect("peer addr should be captured");
    assert!(peer.ip().is_loopback());
    assert_ne!(peer.port(), 0);
}
