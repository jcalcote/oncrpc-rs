# oncrpc-runtime

Use this skill when implementing or changing ONC RPC transport/runtime behavior
in `oncrpc-rs`.

## Read First

Before making runtime changes, read:

- `AGENTS.md`
- `docs/architecture.md`
- `docs/codegen.md` if the runtime change affects generated client/server APIs

## When This Skill Applies

Use it for work in or around:

- `crates/onc-rpc-runtime`
- `crates/onc-rpc-server`
- client call lifecycle behavior
- TCP framing and reassembly in runtime context
- XID tracking and reply correlation
- callback-style request handling
- bind address, timeout, and listener semantics
- TLS layering decisions touching runtime integration

## Workflow

1. Confirm the change belongs in runtime/server code.
   Keep low-level envelope representation in `onc-rpc-wire`.
   Keep generator policy in `onc-rpcgen`.

2. Define the operational behavior before editing.
   Be explicit about:
   - client or server side
   - connection setup/bind behavior
   - timeout behavior
   - request/reply correlation behavior
   - threading or async model impact

3. Keep TCP-first behavior simple and explicit.
   Fixed-port operation is the default target.
   Optional discovery concerns must not shape the core runtime surface.

4. Separate transport failures from protocol failures.
   Runtime code should preserve clear distinctions between:
   - transport/connectivity issues
   - wire decode issues
   - ONC RPC accept/reject outcomes
   - application-level errors

5. Keep generated stubs thin.
   Runtime should provide reusable mechanisms that generated code can call, but
   runtime should not absorb service-specific policy.

6. Preserve testability at module boundaries.
   Avoid designs that require live sockets or ambient runtime state for core
   logic tests when a boundary abstraction would keep the behavior testable.

## Required Tests

Runtime changes should normally include:

- behavioral tests for call/reply sequencing or correlation
- framing/reassembly tests where transport boundaries matter
- timeout or bind-behavior tests when configuration semantics change
- client/server API tests when generated interfaces depend on the runtime shape

If a runtime change affects wire behavior directly, add or update wire-level
tests too.

## Validation

For code changes, run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

Add targeted crate tests for runtime/server changes as they become available.

## Decision Checks

Before finishing, verify:

- the change belongs in runtime/server code rather than wire/generator code
- transport, protocol, and application failure modes remain distinct
- callback and correlation behavior is tested, not just compiled
- optional features did not distort the core runtime API
