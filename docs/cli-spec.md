# oncrpcgen CLI and API Specification

## Purpose

This document specifies the intended command-line interface and library-facing
generation contract for `onc-rpcgen`.

It exists to document the completed synchronous generation contract for:

- CLI option parsing
- build-system integration
- generated synchronous client APIs
- generated synchronous server trait and dispatch APIs

The intent is to keep the Rust design aligned with the feature intent of
`oncrpc4j-rpcgen` for synchronous generation while avoiding Java-specific CLI
and object-model baggage.

## Scope

This specification covers the current `onc-rpcgen` implementation:

- `oncrpcgen` command structure
- generator library entry points
- generation options
- output structure
- generated synchronous client shapes
- generated synchronous server trait and dispatch shapes

This specification does not yet define:

- exact `build.rs` integration API
- asynchronous stub generation
- timeout/per-call auth generated API variants
- rpcbind integration
- TLS-specific generation behavior

## Design Principles

- Keep the CLI thin; substantive behavior belongs in the library.
- Keep generated payload types transport-agnostic where practical.
- Keep generated stubs explicit and testable.
- Prefer typed options structs over long generated parameter lists.
- Prefer Rust-native synchronous API shapes over direct Java callback-style
  emulation.
- Fail closed on unsupported or ambiguous generation inputs.

## Command Structure

The intended top-level CLI is:

```bash
oncrpcgen <command> [options] <x-file>
```

Supported commands:

- `parse`
- `emit types`
- `emit stubs`
- `generate`

### `parse`

Parse an `.x` file and validate that it can be represented by the internal
schema model.

Default output:

- no generated Rust
- success exit status on parse success
- non-zero exit status on failure

Useful options:

- `--emit-ast`
- `--verbose`

Examples:

```bash
oncrpcgen parse proto.x
oncrpcgen parse --emit-ast proto.x
```

### `emit types`

Generate Rust XDR payload types and ONC RPC program/version/procedure constants
from an `.x` file.

When the input file includes other `.x` files, the generator should emit one
Rust output module per loaded IDL file rather than flattening every transitive
declaration into the root file's output.

Examples:

```bash
oncrpcgen emit types proto.x --out-dir generated/
oncrpcgen emit types proto.x --module proto_schema --out-dir generated/
```

### `emit stubs`

Generate Rust client/server stub code from ONC RPC program/version/procedure
declarations in an `.x` file.

Each generated stub file should contain only the `program/version/procedure`
blocks owned by the root input IDL file. Included files may still contribute
type modules, but they should not produce sibling `.stubs.rs` outputs during a
normal root-file generation request.

Examples:

```bash
oncrpcgen emit stubs proto.x --out-dir generated/
oncrpcgen emit stubs proto.x --no-client --out-dir generated/
oncrpcgen emit stubs proto.x --no-server --out-dir generated/
```

### `generate`

Generate the full Rust output for the input `.x` file.

By default this means:

- types
- program/version/procedure constants
- client stubs
- server traits and dispatch glue

If the input schema pulls in additional `.x` files through includes, generation
should emit the reachable sibling Rust type modules needed to represent those
files as separate outputs in the selected `--out-dir`.

Examples:

```bash
oncrpcgen generate proto.x --out-dir generated/
oncrpcgen generate proto.x --no-types --out-dir generated/
oncrpcgen generate proto.x --no-server --verbose --out-dir generated/
```

## CLI Options

The intended cross-command options are:

- `-h`, `--help`
- `--include-dir <dir>`
- `--out-dir <dir>`
- `--module <name>`
- `--no-types`
- `--no-stubs`
- `--no-client`
- `--no-server`
- `--emit-ast`
- `--verbose`

### Option Semantics

#### `-h`, `--help`

Print command help and exit successfully.

The CLI should support both top-level and subcommand-scoped help through the
standard command-line interface behavior.

#### `--include-dir <dir>`

Add an include search directory used when resolving `%#include` references from
`.x` files.

The first implementation should support repeated `--include-dir` flags and use
them together with the input file's parent directory to resolve included `.x`
files.

#### `--out-dir <dir>`

Required for file-emitting commands unless the implementation explicitly
supports stdout-only modes.

Specifies the destination directory for generated Rust output.

For include-aware generation, this directory should receive one generated Rust
type file per loaded IDL module, and stub output only for the root input file
unless a future explicit option is added to request all transitive stubs.

#### `--module <name>`

Optional override for the generated module name derived from the root input
file.

This should be used sparingly. Deterministic schema-derived naming remains the
default.

#### `--no-types`

Suppress XDR type emission.

Relevant only for `generate`.

#### `--no-stubs`

Suppress stub emission entirely.

Relevant only for `generate`.

#### `--no-client`

Suppress synchronous client stub generation.

Relevant for `emit stubs` and `generate`.

#### `--no-server`

Suppress synchronous server trait/dispatch generation.

Relevant for `emit stubs` and `generate`.

#### `--emit-ast`

Emit a deterministic textual view of the parsed schema model.

Useful for debugging parser coverage and snapshot-style testing.

#### `--verbose`

Enable progress-oriented generation diagnostics.

This should be informational output, not a substitute for structured errors.

## Options Intentionally Not Carried Forward

The following `oncrpc4j-rpcgen` options should not be copied directly into the
Rust design:

- `-c`
- `-s`
- `-p` / `-package` as a Java package analogue
- `-ser`
- `-bean`
- `-initstrings`
- `-nobackup`
- `-asynccallback` as the primary async model

Reasons:

- Rust should prefer schema-driven deterministic type and module naming.
- Rust does not need Java bean/serialization toggles.
- Output-file backup behavior is not part of the generator contract.
- Callback-oriented async generation is not the preferred Rust async surface.

## Library Boundary

The CLI must remain a thin wrapper over library entry points.

The intended library-facing generation contract is:

```rust
pub struct GenerateOptions {
    pub module_name: Option<String>,
    pub emit_types: bool,
    pub emit_stubs: bool,
    pub emit_client: bool,
    pub emit_server: bool,
}

pub struct GeneratedModuleOutput {
    pub module_name: String,
    pub types: Option<String>,
    pub stubs: Option<String>,
}

pub struct GeneratedOutputs {
    pub root_module: String,
    pub modules: Vec<GeneratedModuleOutput>,
}

pub fn parse_x_file(...) -> Result<Schema, GeneratorError>;
pub fn emit_rust_types(...) -> Result<String, GeneratorError>;
pub fn emit_rust_stubs(...) -> Result<String, GeneratorError>;
pub fn generate_rust(...) -> Result<GeneratedOutputs, GeneratorError>;
```

The exact names may still evolve, but the shape should remain:

- parse API
- focused emit APIs
- one higher-level generation API with explicit synchronous-generation options

## Output Layout

The generator should logically separate:

- types/constants output
- client output
- server output

The implementation may choose either:

- separate files
- separate modules within one file

but it must do so deterministically.

The current preferred direction is:

- one Rust output file per loaded `.x` file
- deterministic sibling module naming derived from each input file stem
- cross-module Rust references for imported types and constants
- no inlining of unrelated transitive `program` blocks into the root module
- no `.stubs.rs` output for included dependency files during default root-file
  generation

## Generated Client API

The current client generation contract is typed and synchronous.

Examples:

```rust
pub fn proc(&self, arg: Arg) -> Result<Ret, RuntimeError>;
pub fn proc(&self) -> Result<Ret, RuntimeError>;
pub fn proc(&self, arg: Arg) -> Result<(), RuntimeError>;
```

Generated client methods must marshal arguments and replies through
`onc_rpc_xdr::XdrEncode` and `onc_rpc_xdr::XdrDecode`.

## Generated Server API

Server generation currently produces:

- typed synchronous service traits
- dispatch adapters targeting `onc-rpc-server::Dispatch`

Example:

```rust
pub trait BlobServiceV1Service {
    fn blob_null(&self) -> Result<(), DispatchError>;
    fn blob_copy(&self, arg: copy_request_t) -> Result<job_result_t, DispatchError>;
}
```

Generated dispatch glue must:

- decode typed arguments from request payload bytes
- invoke the typed service trait
- encode typed results into reply payload bytes
- map malformed payloads to `DispatchError::GarbageArgs`
- map reply encoding failures to `DispatchError::SystemError`

## Error Policy

The generator must fail closed when:

- typed marshalling cannot be emitted for a recognized XDR construct
- generation would require unresolved cross-file references
- naming would collide without a documented conflict-resolution policy

## Relationship to oncrpc4j

This design intentionally preserves the useful feature intent from
`oncrpc4j-rpcgen`:

- client/server generation toggles
- deterministic code generation from `.x` inputs
- separate type and stub emission

It intentionally does not preserve Java-specific choices such as:

- bean generation
- serializable toggles
- callback-first async design
- class-name override driven architecture

## Implementation Order

The recommended implementation order is:

1. add this contract to the repo as documentation
2. introduce runtime `CallOptions`
3. generate typed sync client methods
4. generate typed sync server traits and dispatch glue
5. generate async variants
6. implement the CLI as a thin wrapper over the library API

This order keeps the generated API contract ahead of the command-line wrapper.
