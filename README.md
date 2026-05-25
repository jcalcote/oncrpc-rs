# oncrpc-rs

Rust workspace for building practical ONC RPC client, server, wire-format, auth, TLS, and code-generation support in Rust.

## Goals

This project aims to provide a focused, reusable Rust implementation of the ONC RPC pieces needed by real systems,
without trying to reproduce every feature of older Java or C ecosystems up front.

Current goals:

- TCP client/runtime support
- UDP client/runtime support
- TCP server/runtime support
- UDP server/runtime support
- `AUTH_NONE` and `AUTH_SYS`
- generated Rust types, client stubs, and server stubs from `.x` definitions
- additive synchronous and asynchronous client/server generation
- optional RPC-over-TLS / STARTTLS support where required by consumers

Non-goals for the initial implementation:

- full `oncrpc4j` parity on day one
- broad framework integration layers
- every historical ONC RPC transport or auth mode
- mandatory `rpcbind` / portmap support for consumers that already use fixed ports or control-plane discovery

## Why `onc-rpcgen` Is Core

For this project, XDR-only support is not enough.

The target `.x` files are not just data schemas; they also define ONC RPC programs, versions, and procedures. A
usable Rust replacement therefore needs integrated generation for both halves of the IDL:

- XDR types: structs, enums, unions, typedefs, variable-length arrays, strings, and opaque values
- ONC RPC surface: program/version/procedure constants, client call stubs, and server dispatch stubs

That makes `onc-rpcgen` a required crate in the workspace, not an optional future convenience layer.

## Workspace Layout

- `crates/onc-rpc-runtime`: transport/runtime primitives for ONC RPC clients
- `crates/onc-rpc-server`: server registration, dispatch, and lifecycle APIs
- `crates/onc-rpc-wire`: owned wire-format types, auth envelopes, and record-marking support
- `crates/onc-rpc-auth`: auth types and helpers for `AUTH_NONE` / `AUTH_SYS`
- `crates/onc-rpc-tls`: TLS and STARTTLS integration points
- `crates/onc-rpc-xdr`: shared XDR encode/decode traits and helpers for generated code
- `crates/onc-rpcgen`: core code generator for XDR types plus ONC RPC client/server stubs
- `crates/onc-rpc-bind`: optional `rpcbind` / portmap support

## Current Status

The workspace now includes:

- owned ONC RPC wire types and TCP record-marking support
- synchronous and asynchronous UDP client/server transports alongside the TCP
  path
- `AUTH_NONE` and `AUTH_SYS` helpers plus server-side auth decoding support
- generated Rust XDR types with `XdrEncode` / `XdrDecode` impls
- per-file `.x` module generation with include-aware loading
- typed synchronous and asynchronous client/server stub generation
- request-level call options with per-call timeout and auth override support
- optional synchronous and asynchronous `rpcbind` lookup/register/unregister
  support for TCP/TCP6 and UDP/UDP6 mappings
- optional direct TLS and STARTTLS client/server transports on top of the
  existing Tokio TCP path
- a working `oncrpcgen` CLI that can generate usable output from real `.x` inputs

The main remaining implementation work is in transport depth, interoperability,
and broader runtime features beyond the current typed sync/async generation
contract.

See [docs/architecture.md](docs/architecture.md) for the current crate-boundary and code-generation direction.
See [docs/codegen.md](docs/codegen.md) for fixture layout, output policy, and generator testing expectations.

## Minimal Example

A tiny end-to-end time service example now lives under
[`examples/time-service/`](examples/time-service).

The IDL is intentionally small:

```xdr
typedef string time_string<64>;

program TIME_SERVICE {
    version TIME_SERVICE_V1 {
        time_string GET_TIME(void) = 1;
    } = 1;
} = 0x31230001;
```

Run the example server and client from the workspace root:

```bash
cargo run -p time-service-example --bin time-server
cargo run -p time-service-example --bin time-client -- 127.0.0.1:4000
```

The full source is in:

- `examples/time-service/src/bin/time-server.rs`
- `examples/time-service/src/bin/time-client.rs`
- `examples/time-service/time_service.x`

The server and client live in separate source files. The core Rust shape is
short enough to read directly:

```rust
use onc_rpc_server::{Program, ServerBuilder, TokioAsyncServerTransport};
use time_service_example::time_service;
use time_service_example::time_service_stubs::time_service::time_service_v1::async_server::{
    TIME_SERVICE_V1Dispatch, TIME_SERVICE_V1Service,
};

struct TimeService;

#[onc_rpc_server::async_trait]
impl TIME_SERVICE_V1Service for TimeService {
    async fn get_time(&self) -> Result<time_service::time_string, onc_rpc_server::DispatchError> {
        Ok("unix-seconds: ...".to_string())
    }
}

let mut server = ServerBuilder::new().with_bind_addr("127.0.0.1:4000".parse()?).build_async();
server.register(
    Program { number: time_service::time_service::PROGRAM, version: time_service::time_service::time_service_v1::VERSION },
    TIME_SERVICE_V1Dispatch::new(TimeService),
)?;
TokioAsyncServerTransport::bind(server).await?.serve().await?;
```

```rust
use onc_rpc_runtime::{AsyncClient, ClientConfig, TokioAsyncClientTransport};
use time_service_example::time_service_stubs::time_service::time_service_v1::async_client::TIME_SERVICE_V1Client;

let config = ClientConfig::new("127.0.0.1:4000".parse()?)
    .with_connect_timeout(std::time::Duration::from_secs(5));
let transport = TokioAsyncClientTransport::connect(&config).await?;
let stub = TIME_SERVICE_V1Client::new(AsyncClient::new(config, transport));
println!("{}", stub.get_time().await?);
```

When a specific call needs different timeout behavior than the client default,
generated client stubs also expose `_with_options(...)` variants that take
`onc_rpc_runtime::CallOptions`.

Those options can also override per-call authentication policy, including
explicit `AUTH_NONE` and `AUTH_SYS` credentials/verifiers.

## TLS and STARTTLS

The workspace now includes optional TLS support in
[`crates/onc-rpc-tls/`](crates/onc-rpc-tls).

What is supported:

- direct TLS client transports for sync and async runtime clients
- direct TLS server transports for sync and async dispatch paths
- STARTTLS upgrade transports that follow the RFC 9289 `AUTH_TLS` probe flow
  on the same TCP connection
- ALPN `sunrpc` negotiation on both client and server TLS configs
- fixed-port TCP operation remains unchanged when TLS is not selected

At a high level:

- use the existing `onc-rpc-runtime` and `onc-rpc-server` client/server types
- swap in TLS or STARTTLS transport types from `onc-rpc-tls`
- provide Rustls client/server configuration through
  `ClientTlsConfig` / `ServerTlsConfig`

STARTTLS support is explicit rather than automatic. Consumers choose the
STARTTLS transport when they want the RFC 9289 probe-and-upgrade flow on a
standard RPC TCP port.

This TLS support currently applies to the TCP transport path. DTLS is not part
of the current implementation.

## Third-Party Dependencies

Current Rust dependencies are intentionally small:

- [`async-trait`](https://docs.rs/async-trait/latest/async_trait/): ergonomic async trait bridge for generated server APIs
- [`tokio`](https://docs.rs/tokio/latest/tokio/): async runtime and networking primitives
- [`bytes`](https://docs.rs/bytes/latest/bytes/): efficient byte buffer handling
- [`clap`](https://docs.rs/clap/latest/clap/): command-line parsing for `oncrpcgen`
- [`thiserror`](https://docs.rs/thiserror/latest/thiserror/): error definitions
- [`tracing`](https://docs.rs/tracing/latest/tracing/): structured instrumentation hooks

The workspace owns its wire-format layer directly in `onc-rpc-wire` rather than depending on a third-party ONC RPC
codec crate. That keeps message layout, record-framing behavior, and future transport requirements under project
control.

Current CI uses:

- [`dtolnay/rust-toolchain`](https://github.com/dtolnay/rust-toolchain)
- [`Swatinem/rust-cache`](https://github.com/Swatinem/rust-cache)

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

## Near-Term Priorities

- continue hardening runtime/server behavior under real interoperability and
  load
- improve `oncrpcgen` CLI help and consumer integration ergonomics
- broaden TLS interoperability and policy coverage
- broaden examples and generated-code polish for downstream consumers
