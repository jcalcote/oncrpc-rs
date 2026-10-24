# Changelog

## 0.3.0 — 2026-10-24

### Added

- Concise generated method documentation covering call options, buffer placement,
  immediate admission and server handlers.
- Shared checked `XidAllocator` namespaces for built-in physical connections,
  including clients constructed from transport clones and explicit raw callers.
- Per-call `CallCancellation` tokens carried by existing `CallOptions`, with
  shared cancellation/wakeup semantics for ordinary and generated clients.
- Typed `Cancelled`, `CallTimeout { duration }`, `WriteTimeout { duration }` and
  `UnsupportedCancellation` runtime errors.
- A [living migration guide](docs/migration.md) with ordinary and caller-owned
  generated-client examples and custom transport guidance.
- Caller-owned RPC receive placement through `ReplyReceiver`, `call_into` and
  `ReceiveOutcome`, with the original destination returned after final access on
  success and failure.
- Segmented immutable request payloads and fixed-segment receive convenience
  APIs, avoiding full RPC payload coalescing on the new path.
- Opt-in per-connection call/byte admission limits. `try_call_into` and `try_call`
  return typed pre-send refusal with the original owners. Callers control wait,
  retry and connection closure; congestion alone does not close the connection.
- Generated typed caller-owned receive and segmented send methods, metadata
  projections, multi-owner opaque arguments and immediate admission variants.
- Separate TCP client/server examples driven by shared `.x` definitions,
  RFC 1813 NFSv3 READ/WRITE/COMMIT behavioral fixtures, deterministic lifecycle and
  framing tests, and minimum-Rust runtime/example checks.

### Breaking changes and lifecycle behavior

- `CallOptions` and `RuntimeError` are non-exhaustive. Downstream options struct
  literals must use builders; exhaustive error matches need a fallback arm.
- Call and hard write deadlines use typed error variants. Code that parsed
  transport-error strings must migrate to typed matching.
- Explicit cancellation or a call deadline during a partial TCP write lets the
  writer finish that frame. A hard write timeout, I/O failure or dropped future
  during a partial write can retire the connection.
- After a complete send, cancellation/deadline/drop detaches the receive
  destination after its last access and drains the remaining or late reply.
  Recover the original owner by canceling explicitly and awaiting the outcome;
  dropping the future abandons its ownership return.
- Custom transport defaults reject explicitly requested cancellation with
  `UnsupportedCancellation` rather than silently ignoring it.

Ordinary generated method signatures and the RPC/XDR wire format are unchanged.
Existing constructors preserve admission without configured ceilings; explicit
limited constructors enable bounds. Admission charges logical request,
descriptor and inline-owner bytes, not process RSS. Ordinary kernel and TLS
copies remain; ordinary generated XDR decoders keep their existing
representations. The minimum supported Rust version remains 1.85.

Cancellation stops local waiting and result delivery. Once the server has
received a request, it may keep executing it; cancellation is neither remote
abort nor evidence that a side effect did not occur. There is no automatic retry.

See [Migration guide](docs/migration.md) and
[Caller-owned RPC buffers](docs/owned-buffers.md) for the complete contract.
