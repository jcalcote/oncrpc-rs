# oncrpc-rs

Rust workspace for building practical ONC RPC client, server, auth, TLS, and code-generation support in Rust.

## Goals

This project aims to provide a focused, reusable Rust implementation of the ONC RPC pieces needed by real systems,
without trying to reproduce every feature of older Java or C ecosystems up front.

Current goals:

- TCP client/runtime support
- TCP server/runtime support
- `AUTH_NONE` and `AUTH_SYS`
- generated Rust client/server stubs from `.x` definitions
- async callback-style request handling
- optional RPC-over-TLS / STARTTLS support where required by consumers

Non-goals for the initial implementation:

- full `oncrpc4j` parity on day one
- broad framework integration layers
- every historical ONC RPC transport or auth mode
- mandatory `rpcbind` / portmap support for consumers that already use fixed ports or control-plane discovery

## Workspace Layout

- `crates/onc-rpc-runtime`: transport/runtime primitives for ONC RPC clients
- `crates/onc-rpc-server`: server registration, dispatch, and lifecycle APIs
- `crates/onc-rpc-auth`: auth types and helpers for `AUTH_NONE` / `AUTH_SYS`
- `crates/onc-rpc-tls`: TLS and STARTTLS integration points
- `crates/onc-rpcgen`: code generator for `.x` service definitions
- `crates/onc-rpc-bind`: optional `rpcbind` / portmap support

## Current Status

This repository currently contains a scaffolded workspace with buildable crate boundaries and placeholder APIs. The
main immediate work is expected in `onc-rpc-runtime`, `onc-rpc-server`, and `onc-rpcgen`.

## Third-Party Dependencies

Current Rust dependencies are intentionally small:

- [`onc-rpc`](https://docs.rs/onc-rpc/latest/onc_rpc/): low-level ONC RPC wire types and codec support
- [`tokio`](https://docs.rs/tokio/latest/tokio/): async runtime and networking primitives
- [`bytes`](https://docs.rs/bytes/latest/bytes/): efficient byte buffer handling
- [`thiserror`](https://docs.rs/thiserror/latest/thiserror/): error definitions
- [`tracing`](https://docs.rs/tracing/latest/tracing/): structured instrumentation hooks

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

- implement TCP record framing and reassembly
- define a stable client call API
- define a server registration and dispatch API
- add initial `.x` parsing and stub generation
- add interoperability tests against existing ONC RPC implementations
