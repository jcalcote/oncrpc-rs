# oncrpc-wire

Use this skill when implementing or changing ONC RPC wire-level behavior in
`oncrpc-rs`.

## Read First

Before making wire-level changes, read:

- `AGENTS.md`
- `docs/architecture.md`
- `docs/codegen.md` if the wire change affects generated APIs or generated
  serialization expectations

## When This Skill Applies

Use it for work in or around:

- `crates/onc-rpc-wire`
- ONC RPC call/reply envelope structures
- auth envelope representation
- record marking and fragmentation semantics
- low-level encode/decode behavior
- wire compatibility tests

## Workflow

1. Confirm the change belongs in `onc-rpc-wire`.
   Keep runtime concerns out of this crate.
   Keep application policy and generated-code policy out of this crate.

2. Define the protocol boundary clearly before editing.
   Be explicit about:
   - which ONC RPC structure or framing rule is being represented
   - whether the change is envelope-level, auth-level, or TCP record-level
   - whether the behavior is fully implemented or intentionally stubbed

3. Preserve strict separation from runtime behavior.
   `onc-rpc-wire` should describe and encode protocol structures, not manage:
   - connection lifecycle
   - retries
   - callbacks
   - timeout policy
   - service registration

4. Fail closed for unsupported wire cases.
   Do not silently accept malformed or ambiguous protocol input.

5. Keep the API explicit.
   Prefer concrete message, reply, marker, and auth types over hidden helper
   behavior that obscures the wire contract.

6. Preserve isolated testability.
   Prefer pure data and codec logic that can be exercised with deterministic
   inputs and outputs without transport setup.

## Required Tests

Wire-level changes should normally include:

- golden tests for encoded values or record markers
- decode/validation tests for malformed input
- round-trip encode/decode tests when serialization support exists

If a change affects generated code expectations, add or update generator-facing
tests as well.

## Validation

For code changes, run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

If `onc-rpc-wire` tests exist for the affected behavior, run them directly as
part of the change.

## Decision Checks

Before finishing, verify:

- the change belongs in `onc-rpc-wire` and not runtime/server/generator code
- the wire contract is explicit and deterministic
- malformed or unsupported inputs fail clearly
- runtime policy did not leak into the wire crate
