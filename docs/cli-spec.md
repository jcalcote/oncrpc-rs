# oncrpcgen CLI and API Specification

## Purpose

This document specifies the intended command-line interface and library-facing
generation contract for `onc-rpcgen`.

It exists to fix the target before implementation spreads across:

- CLI option parsing
- build-system integration
- generated sync and async client APIs
- generated server trait and dispatch APIs
- timeout and per-call auth support

The intent is to keep the Rust design aligned with the feature intent of
`oncrpc4j-rpcgen` while avoiding Java-specific CLI and object-model baggage.

## Scope

This specification covers:

- `oncrpcgen` command structure
- generator library entry points
- generation options
- output structure
- generated sync and async client shapes
- generated server trait and dispatch shapes
- timeout and per-call auth configuration shape

This specification does not yet define:

- exact `build.rs` integration API
- CLI implementation details such as which argument parser crate to use
- exact XDR encode/decode trait names
- rpcbind integration
- TLS-specific generation behavior

## Design Principles

- Keep the CLI thin; substantive behavior belongs in the library.
- Keep generated payload types transport-agnostic where practical.
- Keep generated stubs explicit and testable.
- Prefer typed options structs over long generated parameter lists.
- Prefer Rust-native sync/async API shapes over direct Java callback-style
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
oncrpcgen parse proto.x --emit-ast
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
oncrpcgen emit stubs proto.x --sync --async --timeouts --out-dir generated/
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
oncrpcgen generate proto.x --no-server --sync --async --timeouts --out-dir generated/
```

## CLI Options

The intended cross-command options are:

- `-h`, `--help`
- `--include-dir <dir>`
- `--out-dir <dir>`
- `--module <name>`
- `--no-types`
- `--no-client`
- `--no-server`
- `--parse-only`
- `--sync`
- `--async`
- `--one-way`
- `--timeouts`
- `--per-call-auth`
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

#### `--no-client`

Suppress client stub generation.

Relevant for `emit stubs` and `generate`.

#### `--no-server`

Suppress server stub/trait/dispatch generation.

Relevant for `emit stubs` and `generate`.

#### `--parse-only`

Parse and validate the input schema without writing generated output.

This is equivalent in intent to `oncrpc4j-rpcgen`'s `-parseonly`.

#### `--sync`

Generate synchronous client and server-facing APIs.

If neither `--sync` nor `--async` is specified, the default should be:

- generate sync APIs
- do not generate async APIs yet unless the implementation explicitly decides
  the default includes both

The project should choose one default and document it consistently.

#### `--async`

Generate asynchronous client and server-facing APIs.

This should mean Rust-native async support, not Java callback-style support.

#### `--one-way`

Generate one-way client methods for procedures where the runtime contract
supports fire-and-forget semantics.

This option must not silently change semantics for procedures that require a
reply to preserve protocol correctness.

#### `--timeouts`

Generate options-bearing method variants that accept timeout configuration.

This should not explode default generated signatures by appending timeout
parameters everywhere.

#### `--per-call-auth`

Generate options-bearing method variants that accept per-call authentication
overrides.

As with timeout support, this should be represented via a typed options
struct, not via long positional parameter lists.

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
    pub emit_client: bool,
    pub emit_server: bool,
    pub emit_sync: bool,
    pub emit_async: bool,
    pub emit_one_way: bool,
    pub emit_timeout_overloads: bool,
    pub emit_per_call_auth_overloads: bool,
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
- one higher-level generation API with explicit options

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

The client generation contract should support three layers.

### 1. Raw Client Methods

These expose the lower-level runtime call boundary.

Examples:

```rust
pub fn proc_raw(&self, payload: Bytes) -> Result<CallResponse, RuntimeError>;
pub async fn proc_raw_async(&self, payload: Bytes) -> Result<CallResponse, RuntimeError>;
```

Raw methods are primarily useful for:

- debugging
- transitional integration
- advanced consumers that want full control over payload marshalling

### 2. Typed Client Methods

These are the default ergonomic client surface.

Examples:

```rust
pub fn proc(&self, arg: Arg) -> Result<Ret, RuntimeError>;
pub async fn proc_async(&self, arg: Arg) -> Result<Ret, RuntimeError>;
```

For `void` argument procedures:

```rust
pub fn proc(&self) -> Result<Ret, RuntimeError>;
pub async fn proc_async(&self) -> Result<Ret, RuntimeError>;
```

For `void` return procedures:

```rust
pub fn proc(&self, arg: Arg) -> Result<(), RuntimeError>;
```

### 3. Options-Bearing Client Methods

These provide timeout and per-call auth support without cluttering the default
method signatures.

Examples:

```rust
pub fn proc_with(&self, arg: Arg, options: CallOptions) -> Result<Ret, RuntimeError>;
pub async fn proc_with_async(
    &self,
    arg: Arg,
    options: CallOptions,
) -> Result<Ret, RuntimeError>;
```

The default methods should delegate to these using client defaults.

## Call Options

Timeout and per-call auth support should be represented through a typed options
struct in the runtime layer.

Intended shape:

```rust
pub struct CallOptions {
    pub timeout: Option<Duration>,
    pub auth: Option<OpaqueAuth>,
}
```

This is the Rust replacement for Java-style generated method overloads with
extra positional timeout and auth parameters.

`CallOptions` belongs in the runtime contract, not inside generated schema
modules.

## Generated Server API

Server generation should produce:

- typed service traits
- optional async service traits
- dispatch adapters targeting `onc-rpc-server::Dispatch`

### Typed Sync Service Trait

Example:

```rust
pub trait BlobServiceV1Service {
    fn blob_null(&self) -> Result<(), DispatchError>;
    fn blob_copy(&self, arg: copy_request_t) -> Result<job_result_t, DispatchError>;
}
```

### Typed Async Service Trait

Example intent:

```rust
pub trait BlobServiceV1AsyncService {
    async fn blob_null(&self) -> Result<(), DispatchError>;
    async fn blob_copy(
        &self,
        arg: copy_request_t,
    ) -> Result<job_result_t, DispatchError>;
}
```

The exact async-trait strategy may depend on language and crate choices, but
the generated contract should still target a clearly async service surface.

### Dispatch Adapter

Generated dispatch glue should:

- decode typed arguments from raw payload bytes
- invoke the typed service trait
- encode typed results into reply payload bytes
- map missing procedures and decoding failures into explicit dispatch errors

The adapter should remain thin and deterministic.

## Sync and Async Generation Policy

The generator should support:

- sync-only output
- async-only output
- combined sync and async output

The chosen default must be documented once implementation lands.

Until then, the generator contract should not assume that async support implies
callback-style APIs.

The preferred Rust async surface is:

- `async fn`-style generated methods
- future-returning methods only if required by the surrounding runtime design

Callback-style async generation may be added later, but it should not be the
primary async contract.

## One-Way Generation Policy

If one-way support is enabled:

- one-way methods must be named distinctly or be otherwise unambiguous
- procedures that semantically require replies must not be silently downgraded
- runtime support must define what transport-level success means for a one-way
  call

If the runtime cannot faithfully support one-way semantics yet, generation
must fail closed rather than emit misleading APIs.

## Timeout Generation Policy

Timeout support should generate options-bearing method variants, not duplicate
every method into many positional-parameter forms.

This keeps the Rust API compact and readable while still exposing timeout
control where needed.

## Per-Call Auth Generation Policy

Per-call auth support follows the same rule as timeout support:

- default methods use client-level defaults
- `_with(...)` variants accept `CallOptions`
- generated code must not require per-call auth parameters everywhere

## Error Policy

The generator must fail closed when:

- requested sync/async/one-way modes are not supported by the runtime contract
- typed marshalling cannot be emitted for a recognized XDR construct
- generation would require unresolved cross-file references
- naming would collide without a documented conflict-resolution policy

## Relationship to oncrpc4j

This design intentionally preserves the useful feature intent from
`oncrpc4j-rpcgen`:

- parse-only mode
- client/server generation toggles
- sync/async generation
- timeout support
- per-call auth support

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
