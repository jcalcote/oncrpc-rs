# Code Generation

## Purpose

`onc-rpcgen` is responsible for turning `.x` inputs into:

- Rust XDR payload types
- ONC RPC program/version/procedure constants
- client stubs
- server traits and dispatch glue

This document narrows the operational contract for generator work so future
changes stay aligned.

## Source of Truth

- `.x` files are the source of truth.
- `RFC 4506` constrains XDR type semantics and encoding rules.
- `RFC 5531` constrains ONC RPC program/version/procedure and message-layer
  semantics.
- Real consumer `.x` files are the primary compatibility target.
- Toy schemas are useful for focused cases, but they must not become the main
  generator contract.

## Fixture Layout

Canonical generator fixtures live under `tests/fixtures/`.

The intended layout is:

- `tests/fixtures/xdr/real/`
  Real `.x` inputs copied or mirrored from active consumer schemas.
- `tests/fixtures/xdr/synthetic/`
  Small targeted `.x` fixtures for isolated language or protocol features.
- `tests/fixtures/expected/`
  Expected generated output and snapshots.

Fixture names should stay stable and descriptive. When a real fixture covers
multiple protocol features, prefer keeping it whole instead of splitting it
into toy fragments.

## Output Policy

Generator work must follow one of these models explicitly:

- checked-in generated outputs for selected fixtures
- snapshot-verified generated outputs
- both, if there is a strong reason

For now, the project should assume snapshot-verified outputs plus behavioral
tests, not wholesale checked-in generated trees.

## Naming and Module Policy

Generator changes must preserve a stable policy for:

- Rust module names derived from `.x` filenames
- symbol naming for programs, versions, procedures, structs, enums, and unions
- collision handling for reserved words or duplicate names

Naming rules should be documented in code or tests when first introduced.

## Dependency Policy for Generated Code

Generated code may depend on:

- `onc-rpc-wire`
- `onc-rpc-runtime`
- `onc-rpc-server`
- `onc-rpc-auth`

Generated code should not depend on optional or policy-heavy crates unless
there is an explicit architectural reason. In particular, optional discovery
support such as `onc-rpc-bind` must not leak into core generated APIs.

Generated payload types should remain transport-agnostic where practical.

## Failure Policy

Unsupported or ambiguous generator inputs must fail closed.

Examples of generator work that should error rather than approximate:

- unresolved cross-file type references
- duplicate procedure identifiers within a version
- unsupported or ambiguous union forms
- include resolution that produces conflicting symbols
- incomplete mapping of a recognized IDL construct

Silent partial generation is not acceptable.

## Testing Requirements

Generator changes should extend the test harness in three directions:

1. fixture-based snapshot coverage
2. behavioral tests for emitted Rust code
3. round-trip or interoperability tests where serialization or wire behavior is
   affected

Snapshot tests alone are not sufficient.

## Initial Compatibility Targets

The first compatibility bar should be the active `.x` files used by current
consumers. As the project grows, add broader fixtures only after preserving
that baseline.
