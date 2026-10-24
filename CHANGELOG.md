# Changelog

## 0.2.3 — Unreleased

### Added

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

### Compatibility and behavior

Existing client APIs, generated signatures and `ClientConfig` fields remain
available. Existing constructors preserve admission without configured ceilings;
use the explicit limited constructors to enable bounds. Admission charges
logical request, descriptor and inline-owner bytes, not process RSS.

An owned-receiver timeout after sending starts still retires the connection and
waits for destination quiescence. Preserving that connection by detaching the
receiver and draining late replies is separate work. Ordinary kernel and TLS
copies remain; ordinary generated XDR decoders retain their existing representations.

See [Caller-owned RPC buffers](docs/owned-buffers.md) for the complete contract.
