# Contributing

## Scope

Keep contributions aligned with the repository's current priorities:

- ONC RPC TCP client/runtime support
- ONC RPC TCP server/runtime support
- `AUTH_NONE` and `AUTH_SYS`
- code generation from `.x` definitions
- optional STARTTLS support needed by real consumers

Avoid broad feature work that is not tied to an active consumer without opening an issue first.

## Development

Run the standard checks before submitting changes:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
```

## Pull Requests

- Keep changes narrowly scoped.
- Prefer adding tests with behavior changes.
- Document any wire-format or API compatibility decisions in the PR description.

