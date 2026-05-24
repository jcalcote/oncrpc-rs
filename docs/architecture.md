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
- minimal buffer copying along transport and payload paths
- high concurrency and throughput on shared client and server connections
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
- default per-call timeout and write timeout policy
- request-level call options that can inherit, disable, or override the client
  default timeout
- request/reply correlation
- TCP record framing and reassembly
- synchronous and asynchronous client call handling
- concurrent in-flight request handling on shared client connections

This crate should be TCP-first. Optional discovery layers such as `rpcbind`
must not shape the core runtime abstractions.

Runtime transport should prefer zero-copy or minimal-copy buffer handling where
the protocol shape allows it. In practice that means:

- prefer `bytes::Bytes` / `bytes::BytesMut` ownership through transport paths
- prefer reusable read buffers over repeated allocation
- avoid reconstructing payload buffers when a borrowed or sliced view is
  sufficient
- accept bounded assembly buffering where ONC RPC TCP fragmentation makes it
  unavoidable, but do not introduce extra copies beyond that requirement

Runtime transport should also treat throughput and concurrency as first-class
requirements. In practice that means:

- a single shared client connection must support multiple concurrent in-flight
  requests correlated by XID
- throughput must not depend on client-instance pooling as a workaround for
  hidden single-threaded call paths
- public client types should be safe to share across threads when the
  underlying transport supports it
- synchronous convenience layers must not silently collapse concurrent callers
  onto one serialized execution path
- connection, default per-call reply timeout, and write timeout behavior should
  be configurable through explicit client configuration rather than hidden
  transport defaults
- no default call timeout should remain a supported and explicit choice for
  long-running synchronous or asynchronous operations
- request-level timeout policy should override client defaults explicitly and
  support three states: inherit the client default, disable the timeout for
  that call, or provide a call-specific duration

### `onc-rpc-server`

Owns server-side runtime behavior:

- listener setup
- service registration
- program/version dispatch
- lifecycle management
- integration with the runtime and wire layers
- synchronous and asynchronous dispatch contracts
- concurrent request handling and reply emission over shared connections
- transport runtime sizing and worker concurrency controls

Server registration should center on explicit `{program, version}` ownership and
generated dispatch glue from `onc-rpcgen`.

Server transport should treat concurrency as a first-class requirement. In
practice that means:

- one connection may carry multiple in-flight requests whose replies are
  produced asynchronously
- selector and worker usage should allow many requests and replies to progress
  concurrently rather than funneling all work through a single execution lane
- generated dispatch glue must remain compatible with highly concurrent caller
  behavior in real Hammerspace components
- server-side concurrency and runtime sizing knobs should be surfaced through
  explicit builder/configuration APIs rather than hard-coded transport choices

### `onc-rpc-tls`

Owns optional TLS and STARTTLS integration points. This crate should wrap the
runtime layer rather than redefine it.

### `onc-rpcgen`

Owns `.x`-driven generation for both halves of the protocol:

- XDR types
- XDR serialization and deserialization implementations
- ONC RPC program/version/procedure constants
- client stubs
- server traits and dispatch stubs

This is a core crate, not an optional add-on.

### `onc-rpc-xdr`

Owns shared XDR serialization support used by generated code:

- `XdrEncode` and `XdrDecode` traits
- scalar and collection codec helpers
- opaque/string padding and alignment rules
- fixed and variable array helpers

This crate exists to keep XDR payload encoding distinct from ONC RPC envelope
encoding in `onc-rpc-wire`.

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

For asynchronous generated APIs, the preferred direction is:

- preserve the synchronous APIs unchanged
- add async support as an additive contract rather than replacing sync
- keep async runtime interfaces executor-agnostic at the public API boundary
- use explicit async client and server generation rather than implicit
  background behavior
- prefer separate generated async modules or types over mixing sync and async
  methods on the same generated type

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

It is also important for buffer ownership: ONC RPC transport framing and XDR
payload handling should preserve buffer reuse and avoid unnecessary copying
across the boundary between envelope handling and payload decoding.

## Testability Constraint

Modules should be designed so they are testable in isolation.

That should influence API and implementation choices in practical ways:

- prefer explicit interfaces over hidden global state
- separate pure parsing/encoding logic from transport side effects
- inject boundary dependencies where practical
- keep generated code testable without requiring a live network stack
- make runtime and dispatch behavior observable with deterministic inputs and
  outputs

## Buffer Ownership Constraint

Transport and serialization code should prefer zero-copy or minimal-copy data
flow by default.

This should influence implementation choices in practical ways:

- favor `Bytes` / `BytesMut` over `Vec<u8>` when shared slicing or freezing is
  useful
- reuse buffers across read and write operations where safe
- keep fragmented-record reassembly to the minimum necessary copy boundary
- avoid converting between owned byte containers gratuitously
- treat unnecessary payload copying as a design issue, not just a micro-
  optimization opportunity

## Thread-Safety Constraint

Client and server runtime types should be designed to be safely shareable across
threads when their public contracts imply shared use.

This should influence API and implementation choices in practical ways:

- shared client instances must not require external pooling merely to obtain
  concurrent request throughput
- request correlation state must remain correct under concurrent access
- write-side serialization should protect wire integrity without serializing
  independent request lifecycles unnecessarily
- server-side dispatch and reply paths should preserve correctness under many
  concurrent in-flight operations

## Async Contract

The project should treat asynchronous support as a first-class, additive layer
on top of the current synchronous contract.

The current preferred async design is:

- `onc-rpc-runtime` keeps its synchronous transport contract and adds a parallel
  async client transport contract
- `onc-rpc-server` keeps its synchronous dispatch contract and adds a parallel
  async dispatch contract
- generated async client stubs target the async runtime contract
- generated async server traits and dispatch glue target the async server
  contract
- generated sync and async APIs coexist rather than one mode being derived
  implicitly from the other

For generated async server traits, the preferred implementation strategy is:

- use `async fn` in the generated trait surface for readability
- use the `async-trait` crate to provide the current implementation bridge
- keep the public contract explicit about async behavior rather than exposing
  boxed-future-heavy signatures by default

This is a pragmatic design choice, not a statement that `async-trait` must be
used forever. If native trait async support evolves to the point that the
project can remove that dependency without degrading ergonomics or testability,
that change can be considered later as an intentional API evolution.

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
