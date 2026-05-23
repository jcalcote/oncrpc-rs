# oncrpc-rs

Rust workspace for building practical ONC RPC client, server, wire-format, auth, TLS, and code-generation support in Rust.

## Goals

This project aims to provide a focused, reusable Rust implementation of the ONC RPC pieces needed by real systems,
without trying to reproduce every feature of older Java or C ecosystems up front.

Current goals:

- TCP client/runtime support
- TCP server/runtime support
- `AUTH_NONE` and `AUTH_SYS`
- generated Rust types, client stubs, and server stubs from `.x` definitions
- async callback-style request handling
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
- `crates/onc-rpcgen`: core code generator for XDR types plus ONC RPC client/server stubs
- `crates/onc-rpc-bind`: optional `rpcbind` / portmap support

## Current Status

This repository currently contains a scaffolded workspace with buildable crate boundaries and placeholder APIs. The
main immediate work is expected in `onc-rpc-runtime`, `onc-rpc-server`, and especially `onc-rpcgen`.

See [docs/architecture.md](docs/architecture.md) for the current crate-boundary and code-generation direction.
See [docs/codegen.md](docs/codegen.md) for fixture layout, output policy, and generator testing expectations.

## Third-Party Dependencies

Current Rust dependencies are intentionally small:

- [`tokio`](https://docs.rs/tokio/latest/tokio/): async runtime and networking primitives
- [`bytes`](https://docs.rs/bytes/latest/bytes/): efficient byte buffer handling
- [`thiserror`](https://docs.rs/thiserror/latest/thiserror/): error definitions
- [`tracing`](https://docs.rs/tracing/latest/tracing/): structured instrumentation hooks

The workspace owns its wire-format layer directly in `onc-rpc-wire` rather than depending on a third-party ONC RPC
codec crate. That keeps message layout, record-framing behavior, and future transport requirements under project
control.

Planned foundational dependency work for code generation:

- evaluate an existing XDR parser/codegen crate such as `xdrgen` for the XDR type-generation layer
- add or build the missing ONC RPC `program/version/procedure` generation in `onc-rpcgen`

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
- add initial `.x` parsing, type generation, and client/server stub generation
- add interoperability tests against existing ONC RPC implementations
