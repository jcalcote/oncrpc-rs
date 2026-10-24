# Migration guide

This is the migration history for `oncrpc-rs`. Add a section for each new
release transition and retain the earlier sections. When skipping releases,
apply the relevant sections in version order.

## 0.2 to 0.3

Version **0.3.0** adds explicit per-call cancellation and
changes how TCP calls release receive buffers after cancellation or a deadline.
The RPC/XDR wire format and ordinary generated method signatures are unchanged.
The minimum supported Rust version remains **1.85**.

Update the workspace crates together. Published dependencies should select
`0.3`; local path dependencies need compatible version requirements as well.
Regenerate checked-in protocol code when adopting the generated caller-owned
methods. Existing ordinary stubs already forward `CallOptions`; they do not need
new cancellation-specific methods.

### Source changes

`CallOptions` and `RuntimeError` are now `#[non_exhaustive]`. Construct options
with builders, and keep a fallback arm when matching runtime errors.

Before:

```rust
let options = CallOptions {
    timeout: CallTimeout::Duration(Duration::from_secs(30)),
    credentials: None,
    verifier: None,
};
```

After:

```rust
use onc_rpc_runtime::CallOptions;
use std::time::Duration;

let options = CallOptions::new().with_timeout(Duration::from_secs(30));
```

The existing authentication builders and `without_timeout()` remain available.
An omitted cancellation token preserves the normal no-explicit-cancellation
behavior. A call deadline still applies if configured on the call or client.

Call and write deadlines now have typed errors rather than requiring callers to
interpret transport-error text:

```rust
use onc_rpc_runtime::RuntimeError;

match result {
    Ok(value) => { /* use value */ }
    Err(RuntimeError::Cancelled) => { /* local cancellation completed */ }
    Err(RuntimeError::CallTimeout { duration }) => { /* call deadline expired */ }
    Err(RuntimeError::WriteTimeout { duration }) => { /* hard write budget expired */ }
    Err(RuntimeError::UnsupportedCancellation) => { /* custom transport lacks support */ }
    Err(error) => { /* other or future error variants */ }
}
```

These variants describe the local operation. After sending has begun, none of
them proves that the remote procedure was not executed. The library does not
automatically retry, abort the server's procedure, or undo its effects.

### Ordinary typed and generated calls

The time-service example's ordinary generated call remains:

```rust
let time = client.get_time().await?;
```

To make that same call explicitly cancellable, use its existing options method:

```rust
use onc_rpc_runtime::{CallCancellation, CallOptions};

let cancellation = CallCancellation::new();
let controller = cancellation.clone();
let options = CallOptions::new().with_cancellation(cancellation);
let call = client.get_time_with_options(&options);
// A task holding `controller` can call controller.cancel() when needed.
let result = call.await;
```

The same options reach ordinary `Client`/`AsyncClient` calls and generated
`*_with_options` methods. There are no additional generated `*_cancel` methods.
Synchronous calls can be canceled from another thread holding a token clone.

`CallCancellation` exposes `cancel()`, `is_cancelled()` and async `cancelled()`.
Clones share one cancellation scope, including wakeups. Cancellation is sticky;
a canceled token stays canceled, so create a new token for a new scope. Token
equality compares that shared identity, not whether two tokens happen to have
the same current canceled state.

### Caller-owned receive buffers

For code written against the earlier caller-owned API draft, the
connection-level recovery pattern was:

```rust
transport.close();
let outcome = call.await; // recover the destination after connection cleanup
```

In 0.3, use per-call cancellation and keep awaiting the call. A fully sent TCP
operation releases its destination after the reader's last access and drains a
remaining or late reply. Other calls can keep using the shared connection.

The following helper uses the checked-in owned-buffer example's generated echo
interface. Its buffer type is generic; any initialized writable owner satisfying
the stated bounds can be returned intact on success or failure:

```rust
use bytes::Bytes;
use onc_rpc_runtime::{
    CallCancellation, CallOptions, OwnedRpcTransport, TypedReceiveOutcome,
};
use owned_buffers_example::{
    buffer_echo::echo_dataMetadata,
    buffer_echo_stubs::buffer_echo::buffer_echo_v1::async_client::BUFFER_ECHO_V1Client,
};

async fn echo_until_shutdown<B>(
    client: &BUFFER_ECHO_V1Client<OwnedRpcTransport>,
    data: Bytes,
    buffer: B,
    shutdown: CallCancellation,
) -> TypedReceiveOutcome<echo_dataMetadata, B>
where
    B: AsMut<[u8]> + Send + 'static,
{
    let cancellation = CallCancellation::new();
    let options = CallOptions::new().with_cancellation(cancellation.clone());
    let call = client.echo_into(data, buffer, &options);
    tokio::pin!(call);

    tokio::select! {
        outcome = &mut call => outcome,
        () = shutdown.cancelled() => {
            cancellation.cancel();
            call.await // wait for actual destination quiescence
        }
    }
}
```

Inspect `outcome.result` before using received bytes. `outcome.buffer` is the
original owner even if the result is `Cancelled`, a timeout, a transport failure,
or a decode error. Cancellation can race with completion; a call that already
completed may return its successful result instead.

Generated `try_*` methods keep their immediate-admission contract. Their accepted
future uses the same cancellation options; a pre-send refusal still returns the
typed request and original receive owner for an application-controlled retry.

Dropping a fully sent call now abandons result delivery and releases its receive
destination after the reader's last access, without waiting for a late reply.
The dropped future cannot return its owner. To recover that owner, explicitly
cancel and await the outcome instead. `close()` and `wait_closed()` alone still
do not establish per-operation quiescence.

### Deadlines, framing and transport differences

| Situation | 0.3 behavior |
| --- | --- |
| Cancellation/deadline before any request bytes are written | Stop the unsent call and release its owners; keep the connection. |
| Explicit cancellation or call deadline during a TCP frame write | Finish the frame before releasing send owners; do not leave a truncated frame for the next writer. |
| Cancellation/deadline after the complete TCP request is sent | Detach the receive destination after its last access and drain the remaining or late reply without closing a healthy connection. |
| Hard write timeout, I/O failure, or future dropped during a partial TCP write | The shared connection can be retired to preserve framing integrity. |
| UDP cancellation or deadline | Stop local waiting; there is no TCP frame to complete. The sent datagram may still execute remotely. |

A call deadline is a logical deadline. Finishing an active TCP frame or stopping
an active receive access can delay the outcome beyond it. Set
`ClientConfig::with_write_timeout(duration)` when physical write cleanup needs a
finite bound. If the peer stops accepting bytes and no write timeout is set,
finishing a partial request can wait indefinitely. A hard write timeout can fail
other calls sharing that connection; per-call cancellation alone does not
require that connection-wide action. Local bind/connect and TLS handshake
budgets keep their existing setup-error behavior; the new typed call and write
timeout variants describe an established call's lifecycle.

For a completion/cancellation race, act on the returned result. Retrying safely
requires application knowledge of procedure idempotence or a remote request
identifier; the runtime does not infer it.

### Shared connection XIDs

Built-in transport clones now expose the same `XidAllocator` to their clients.
Constructing multiple synchronous or asynchronous clients around one physical
connection no longer creates independent XID sequences. This matters after
cancellation: a late reply must not match a new operation that accidentally
reused its XID.

When mixing raw messages with typed calls, obtain raw XIDs from the connection:

```rust
let xid = transport.xid_allocator().allocate()?;
// Use `xid` in the raw RPC envelope for this one operation.
```

Do not deliberately reuse a manually assigned XID on that connection, even if
the original operation was canceled or timed out. The checked allocator never
wraps; establish another connection on exhaustion.

Custom `ClientTransport` and `AsyncClientTransport` implementations can override
`xid_allocator() -> Option<XidAllocator>`. Return clones of one allocator when
multiple transport/client wrappers share a physical connection, or delegate to
the underlying provided transport's allocator. The default `None` retains the
client-local fallback for transports that do not share a connection.

### Custom transports

The provided TCP, TLS/STARTTLS and UDP transports implement the cancellation
contract through their existing options-aware calls. For custom
`ClientTransport` or `AsyncClientTransport` implementations, the default
`call_with_options` implementation rejects an explicitly supplied, uncanceled
token with `RuntimeError::UnsupportedCancellation`. A token already canceled
returns `RuntimeError::Cancelled` without starting the call. The default does
not silently ignore a cancellation request.

A custom transport that overrides `call_with_options` must handle the token and
document its cleanup boundary, or explicitly return `UnsupportedCancellation`.
If it delegates to a provided transport, forward the original options. No token
was added to the lower-level required `call` method, so implementations that
support only the original API can retain that method.

See [the ownership contract](owned-buffers.md) for admission, buffer lifetimes
and byte accounting, and [the generated API contract](cli-spec.md#generated-caller-owned-client-api)
for supported `.x` layouts and multi-owner arguments.
