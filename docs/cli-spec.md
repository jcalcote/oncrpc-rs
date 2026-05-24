# oncrpcgen CLI and API Specification

## Purpose

This document specifies the intended command-line interface and library-facing
generation contract for `onc-rpcgen`.

It exists to document the completed synchronous and asynchronous generation
contract for:

- CLI option parsing
- build-system integration
- generated synchronous and asynchronous client APIs
- generated synchronous and asynchronous server trait and dispatch APIs

The intent is to keep the Rust design aligned with the feature intent of
`oncrpc4j-rpcgen` while avoiding Java-specific CLI and object-model baggage.

## Scope

This specification covers the current `onc-rpcgen` implementation:

- `oncrpcgen` command structure
- generator library entry points
- generation options
- output structure
- generated synchronous and asynchronous client shapes
- generated synchronous and asynchronous server trait and dispatch shapes

This specification does not yet define:

- exact `build.rs` integration API
- timeout/per-call auth generated API variants
- rpcbind integration
- TLS-specific generation behavior

It also records the additive async contract now implemented by
`onc-rpc-runtime`, `onc-rpc-server`, and `onc-rpcgen`.

## Design Principles

- Keep the CLI thin; substantive behavior belongs in the library.
- Keep generated payload types transport-agnostic where practical.
- Keep generated stubs explicit and testable.
- Prefer typed options structs over long generated parameter lists.
- Prefer Rust-native synchronous API shapes over direct Java callback-style
  emulation.
- Preserve synchronous APIs when adding async support; async should be
  additive.
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
- synchronous and asynchronous client stubs
- synchronous and asynchronous server traits and dispatch glue

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

Suppress client stub generation, including both synchronous and asynchronous
client modules.

Relevant for `emit stubs` and `generate`.

#### `--no-server`

Suppress server trait/dispatch generation, including both synchronous and
asynchronous server modules.

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
- one higher-level generation API with explicit client/server generation
  options

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

The current client generation contract is typed and additive:

- typed synchronous client modules
- typed asynchronous client modules

Examples:

```rust
pub fn proc(&self, arg: Arg) -> Result<Ret, RuntimeError>;
pub fn proc(&self) -> Result<Ret, RuntimeError>;
pub fn proc(&self, arg: Arg) -> Result<(), RuntimeError>;
```

Generated client methods must marshal arguments and replies through
`onc_rpc_xdr::XdrEncode` and `onc_rpc_xdr::XdrDecode`.

The intended direction is:

- keep the synchronous client modules and types
- add explicit async client modules alongside them
- target an executor-agnostic async runtime contract at the public API boundary
- avoid collapsing sync and async methods onto the same generated client type

Illustrative shape:

```rust
pub mod client {
    pub struct BlobServiceV1Client<T> { /* ... */ }

    impl<T> BlobServiceV1Client<T> {
        pub fn blob_copy(
            &self,
            arg: copy_request_t,
        ) -> Result<job_result_t, RuntimeError> {
            /* ... */
        }
    }
}

pub mod async_client {
    pub struct BlobServiceV1Client<T> { /* ... */ }

    impl<T> BlobServiceV1Client<T> {
        pub async fn blob_copy(
            &self,
            arg: copy_request_t,
        ) -> Result<job_result_t, RuntimeError> {
            /* ... */
        }
    }
}
```

## Generated Server API

Server generation currently produces:

- typed synchronous service traits and dispatch adapters
- typed asynchronous service traits and dispatch adapters

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

The intended direction is:

- keep the existing generated synchronous server modules and traits
- add explicit async server modules alongside them
- generate async server traits using `async fn`
- use `async-trait` as the current implementation bridge for generated async
  server traits
- target a parallel async dispatch contract in `onc-rpc-server` rather than
  replacing the synchronous dispatch contract

Illustrative shape:

```rust
pub mod async_server {
    #[async_trait::async_trait]
    pub trait BlobServiceV1Service {
        async fn blob_copy(
            &self,
            arg: copy_request_t,
        ) -> Result<job_result_t, DispatchError>;
    }

    pub struct BlobServiceV1Dispatch<T> { /* ... */ }
}
```

This design favors readability and straightforward consumer ergonomics over a
future-returning trait surface. If the project later finds a better way to
express the same contract without `async-trait`, that should be treated as an
intentional API evolution rather than an implicit drift.

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

## Implementation Status

The currently implemented contract covers:

1. deterministic `.x` parsing
2. per-file Rust type generation
3. generated XDR serializers
4. typed synchronous client and server stubs
5. additive typed asynchronous client and server stubs
6. a thin CLI over the library generation API

Future work should build on this contract rather than redefining it.
