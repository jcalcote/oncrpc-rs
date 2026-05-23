# Architecture

## Purpose

`oncrpc-rs` is intended to provide the ONC RPC functionality needed by real
consumers that rely on `.x` IDL files defining both XDR data types and ONC RPC
program/version/procedure interfaces.

The project is intentionally shaped around that full requirement:

- owned ONC RPC wire-format support
- TCP-first client and server runtime support
- `AUTH_NONE` and `AUTH_SYS`
- code generation from `.x` files into Rust types and client/server stubs

## Design Priorities

Current architectural priorities are:

- fidelity to the existing `.x` files used by consumers
- predictable, explicit wire behavior
- generated code that is practical to call from application code
- modules that are directly testable in isolation
- narrow, well-separated crate responsibilities

The initial target is not complete parity with every historical ONC RPC stack.
The target is a usable Rust implementation for real consumers with room to
expand later.

## Specification References

The primary protocol references for this project are:

- `RFC 5531`: ONC RPC Version 2
- `RFC 4506`: XDR
- `RFC 1833`: rpcbind protocol
- `RFC 9289`: RPC-over-TLS

Implementation should prefer RFC-defined behavior over invented local
conventions. If a compatibility requirement or consumer behavior requires a
deliberate divergence, document that divergence explicitly in code comments,
tests, or change descriptions as appropriate.

## Crate Boundaries

### `onc-rpc-wire`

Owns low-level ONC RPC envelope semantics:

- message identifiers
- program/version/procedure identifiers
- auth envelope representation
- call and reply header structures
- TCP record-marking primitives

This crate should stay focused on protocol representation and low-level
encoding/decoding concerns. It should not absorb runtime policy, connection
management, or code generation.

### `onc-rpc-auth`

Owns auth-specific data validation and helpers, especially for:

- `AUTH_NONE`
- `AUTH_SYS`

Wire-compatible auth values may appear in `onc-rpc-wire`, but construction,
validation, and helper APIs belong here.

### `onc-rpc-runtime`

Owns client-side transport/runtime behavior:

- TCP connection setup
- local bind support
- connect timeouts
- request/reply correlation
- TCP record framing and reassembly
- callback-style async request handling

This crate should be TCP-first. Optional discovery layers such as `rpcbind`
must not shape the core runtime abstractions.

### `onc-rpc-server`

Owns server-side runtime behavior:

- listener setup
- service registration
- program/version dispatch
- lifecycle management
- integration with the runtime and wire layers

Server registration should center on explicit `{program, version}` ownership and
generated dispatch glue from `onc-rpcgen`.

### `onc-rpc-tls`

Owns optional TLS and STARTTLS integration points. This crate should wrap the
runtime layer rather than redefine it.

### `onc-rpcgen`

Owns `.x`-driven generation for both halves of the protocol:

- XDR types
- ONC RPC program/version/procedure constants
- client stubs
- server traits and dispatch stubs

This is a core crate, not an optional add-on.

### `onc-rpc-bind`

Owns optional `rpcbind` / portmap support. It is intentionally outside the core
runtime path because current consumers can operate with fixed ports or
control-plane supplied port information.

## IDL Contract

The working assumption is that `.x` files remain the source of truth.

That means `onc-rpcgen` must eventually own:

- parsing of XDR and ONC RPC declarations
- cross-file reference resolution
- include handling compatible with the input files in use
- deterministic Rust output

Generated code should be based on a stable contract, not ad hoc per-service
generation.

## Generated Code Contract

The generated API surface should be decided intentionally before deep parser
work expands. At minimum, generated code needs a consistent answer for:

- Rust naming for programs, versions, procedures, and types
- mapping of XDR unions, optional data, variable arrays, strings, and opaque
  values into Rust types
- client API shape for request/response calls
- server trait shape for handler implementations
- how generated code depends on runtime and wire crates

The preferred direction is:

- generated types remain as transport-agnostic as practical
- generated stubs depend on the shared runtime/wire crates
- server-side generation emits explicit dispatch traits rather than hidden
  reflection-like machinery

The generator should also follow these operational rules:

- output must be deterministic for a given `.x` input
- unsupported or ambiguous constructs must fail closed
- generated code should avoid embedding retry, discovery, or application policy
- intentional generated API shape changes should be called out explicitly

## XDR and ONC RPC Separation

XDR concerns and ONC RPC envelope concerns are related but distinct.

The project should keep them separated conceptually and in code:

- XDR defines payload types and payload serialization rules
- ONC RPC defines call/reply envelopes, auth wrappers, and transport framing

This separation is important both for crate structure and for generator design.

## Testability Constraint

Modules should be designed so they are testable in isolation.

That should influence API and implementation choices in practical ways:

- prefer explicit interfaces over hidden global state
- separate pure parsing/encoding logic from transport side effects
- inject boundary dependencies where practical
- keep generated code testable without requiring a live network stack
- make runtime and dispatch behavior observable with deterministic inputs and
  outputs

## Error Model

The public API should distinguish at least these classes of failure:

- wire decode/encode errors
- transport/runtime failures
- ONC RPC accept/reject reply failures
- application-level procedure failures

Those categories should not collapse into one generic error type, especially in
generated client stubs.

## Compatibility Strategy

Compatibility should be prioritized in this order:

1. the `.x` files and behavior required by active consumers
2. interoperability with established ONC RPC peers
3. broader protocol completeness

That ordering keeps the project aligned with actual integration needs.

## Testing Strategy

The project should maintain at least three layers of tests:

1. wire-level golden tests for record markers, headers, auth envelopes, and
   call/reply encoding
2. code-generation snapshot tests using real `.x` files
3. interoperability tests against external ONC RPC implementations

The real `.x` files used by consumers should become first-class fixtures for
`onc-rpcgen`.

The canonical fixture layout lives under `tests/fixtures/`, with separate
spaces for real inputs, synthetic focused inputs, and expected outputs.

Snapshot coverage alone is not sufficient for generator work. Generator changes
should also carry behavioral tests for emitted Rust code, especially when
adding new IDL construct support or changing generated client/server behavior.

Changes that affect wire compatibility or XDR serialization should include
round-trip encode/decode tests in addition to snapshot validation.

## Near-Term Decisions

Before implementation grows much further, the project should settle:

- the exact generated client API shape
- the exact generated server trait/dispatch shape
- the boundary between wire-level types and XDR serialization code
- the initial fixture and snapshot-testing approach for `.x` inputs
