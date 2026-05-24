---
apply: always
---

# Agent Instructions

These instructions apply to the entire `oncrpc-rs` repository.

## Start Here

- Read [docs/architecture.md](docs/architecture.md) before making structural changes.
- For `.x` parsing or generator work, also read
  [`.ai/skills/oncrpc-codegen/SKILL.md`](.ai/skills/oncrpc-codegen/SKILL.md).
- For wire-level protocol work, also read
  [`.ai/skills/oncrpc-wire/SKILL.md`](.ai/skills/oncrpc-wire/SKILL.md).
- For runtime or server transport work, also read
  [`.ai/skills/oncrpc-runtime/SKILL.md`](.ai/skills/oncrpc-runtime/SKILL.md).
- Keep the project aligned with the architecture document's crate boundaries,
  generator contract, and testing strategy.

## Current Priorities

- owned ONC RPC wire-format support
- TCP-first client/runtime support
- TCP-first server/runtime support
- `AUTH_NONE` and `AUTH_SYS`
- full `.x`-driven generation of XDR types and ONC RPC client/server stubs

## Design Constraints

- Treat `.x` files as the source of truth for generated protocol code.
- Treat the relevant RFCs as the normative protocol constraint unless a
  compatibility-driven exception is documented.
- Design modules so they are directly testable in isolation.
- Keep `onc-rpc-wire` focused on wire-format concerns, not runtime policy.
- Keep XDR payload concerns distinct from ONC RPC envelope concerns.
- Do not let optional features such as `rpcbind` shape the core runtime API.
- Prefer explicit generated traits and dispatch code over hidden or reflective
  behavior.

## Specification References

- `RFC 5531`: ONC RPC Version 2
- `RFC 4506`: XDR
- `RFC 1833`: rpcbind protocol
- `RFC 9289`: RPC-over-TLS

## Generated Code Rules

- Keep generated output deterministic for a given `.x` input.
- Treat unsupported or ambiguous IDL constructs as hard errors.
- Keep generated payload types transport-agnostic where practical.
- Generate explicit client stubs, server traits, and dispatch glue.
- Do not embed retry, discovery, or business policy in generated code.
- Validate generator behavior against real `.x` fixtures and snapshot tests.
- Document any intentional generated API shape change.

## Generated Code Testing

- Snapshot tests alone are not sufficient for generator changes.
- New generator features must include behavioral tests for emitted Rust code.
- New IDL construct support must include fixture coverage using real or
  representative `.x` inputs.
- Changes affecting wire or XDR compatibility must include round-trip
  encode/decode tests.
- Generated client and server stubs should be exercised with compile-time or
  runtime behavior tests where practical.

## Validation

For code changes, run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

If a change affects wire layout, code generation, or public API shape, document
the decision in the commit message or PR description.

## Commit Messages

- Write commit messages with enough detail to explain the substantive change.
- A clear subject line plus a short body is preferred when more context is
  needed.
- When a commit message has a body, wrap it to 80 columns for readability.
- Multi-line commit messages must contain literal newline characters. Do not
  pass escaped `\n` sequences that will appear verbatim in the stored commit
  message.
- Prefer commit-message entry methods that preserve real line breaks, such as a
  commit editor, `git commit -F <file>`, or a heredoc-backed `git commit
  --amend -F -`.
- Do not expand commit messages into long multi-paragraph narratives unless the
  change genuinely requires that level of explanation.
