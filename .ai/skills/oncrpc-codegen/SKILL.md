# oncrpc-codegen

Use this skill when implementing or changing `.x` parsing, XDR type generation,
ONC RPC client/server stub generation, or nearby fixture/test infrastructure in
`oncrpc-rs`.

## Read First

Before making codegen changes, read:

- `AGENTS.md`
- `docs/architecture.md`
- `docs/codegen.md`

These remain the source of truth for architecture and policy. This skill is the
execution workflow.

## When This Skill Applies

Use it for work in or around:

- `crates/onc-rpcgen`
- `tests/fixtures/`
- generated client/server API shape
- generator snapshot or behavioral tests
- mapping `.x` constructs into Rust types or stubs

## Workflow

1. Confirm the change belongs in `onc-rpcgen`.
   Use `onc-rpc-wire` only for ONC RPC envelope and framing concerns.
   Use runtime/server/auth crates only for runtime behavior, not generator-only
   policy.

2. Choose the right fixture first.
   Prefer a real fixture under `tests/fixtures/xdr/real/` when it covers the
   behavior being added or fixed.
   Use `tests/fixtures/xdr/synthetic/` only when a focused isolated schema is
   clearer than a real one.

3. Define the expected contract before editing generator logic.
   Be explicit about:
   - naming
   - module placement
   - generated type shape
   - generated client API shape
   - generated server trait/dispatch shape
   - failure mode for unsupported inputs

4. Fail closed for unsupported or ambiguous constructs.
   Do not silently skip, partially generate, or approximate unresolved input.

5. Keep generated payload types transport-agnostic where practical.
   Generated stubs may depend on shared runtime crates, but payload types should
   not absorb runtime policy unless there is a clear reason.

6. Keep generated code thin.
   Do not embed retries, discovery, or application business logic in emitted
   code.

7. Preserve direct testability.
   Generated code should remain straightforward to compile-test and behavior-test
   without requiring a live network stack.

## Required Tests

Generator changes should usually add more than one test kind.

Required expectations:

- snapshot-style verification for generated output shape
- behavioral tests for emitted Rust behavior
- fixture coverage for every new IDL construct or parser path

Additionally required when applicable:

- round-trip encode/decode tests for XDR or wire-compatibility changes
- client/server stub behavior tests for generated API changes

Snapshot-only generator changes are not sufficient.

## Validation

For code changes, run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
cargo test -p onc-rpcgen
```

If the change affects shared runtime or wire behavior, expand validation to the
relevant crate tests as well.

## Decision Checks

Before finishing, verify:

- the fixture choice matches the change
- generated output remains deterministic
- unsupported inputs fail explicitly
- no optional crate leaked into core generated APIs
- the new tests exercise behavior, not only file diffs

## Notes for Future Agents

- Real consumer `.x` files are the primary compatibility target.
- Do not invent a second formatting or style authority for Rust beyond the
  standard project tooling.
- If generator behavior changes public API shape, call it out explicitly in the
  commit message or PR description.
