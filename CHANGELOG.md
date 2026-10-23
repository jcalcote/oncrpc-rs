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
- A standalone owned-buffer/admission example, deterministic lifecycle and
  framing tests, minimum-Rust runtime checks, and executable documentation in CI.

### Compatibility and behavior

Existing client APIs, generated signatures and `ClientConfig` fields remain
available. Existing constructors preserve admission without configured ceilings;
use the explicit limited constructors to enable bounds. Admission charges
logical request, descriptor and inline-owner bytes, not process RSS.

An owned-receiver timeout after sending starts still retires the connection and
waits for destination quiescence. Preserving that connection by detaching the
receiver and draining late replies is separate work. Ordinary kernel and TLS
copies remain; generated XDR decoders retain their existing representations.

See [Caller-owned RPC buffers](docs/owned-buffers.md) for the complete contract.
