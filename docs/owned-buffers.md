# Caller-owned RPC buffers

The optional `OwnedRpcTransport::call_into`, `call_segmented` and admission APIs are additive
in **0.2.3**.
Existing `AsyncClientTransport`, generated clients, synchronous clients and
`RpcMessage` representations remain available without source migration. This
change prepares a library release; it does not publish crates or create a tag.

## Ownership and wire contract

A `SegmentedRequest` contains an RPC call envelope with an empty payload and a
`Vec<Bytes>` containing its complete XDR argument stream. The envelope carries
program, version, procedure, credentials, verifier and a caller-assigned XID.
That XID must not be reused on the same physical connection, including after a
cancellation or timeout. Use a monotonically allocated XID and replace the
connection on exhaustion, as the existing `AsyncClient` does.

`Bytes::from_owner` can retain a custom immutable allocation without copying it.
The writer holds its segments through all partial writes. It emits a single
last-fragment marker followed by the encoded envelope and arguments. At most
64 `IoSlice` entries are constructed on the stack per write. Streams without
vectored support, including some TLS adapters, use their scalar fallback rather
than coalescing an RPC-sized payload.

`call_into` transfers an application-owned `ReplyReceiver` to the receive path.
Its `buffer()` method supplies the next initialized writable slice; an empty
slice means the receiver is full. After a successful read, `advance(bytes)`
receives the actual count before the next slice is requested. The receiver can
parse a small application prefix, then expose a destination range for the data
and a separate range for padding. Fragment boundaries and short socket reads do
not change this contract. Errors from either receiver callback retire the
connection.

`ReceiveOutcome<R>` always returns the original receiver, the RPC envelope or
error, and the number of XDR result bytes placed in its slices. Return is a
quiescence boundary: the library no longer accesses that receiver. For example,
a consumer can parse a variable-length result header into small control storage
and append successive short reads directly to the remaining tail of the same
admitted allocation. No payload relocation or replacement allocation is needed.
The application must still validate its result's declared lengths and completion
state; reaching the end of an RPC record does not validate application XDR.

`call_segmented` is a convenience adapter taking `Vec<B>`, where
`B: AsMut<[u8]> + Send + 'static`, and filling the buffers in order. Buffer lengths
represent writable capacity. This adapter returns the original buffers on
success and discards them on error; use `call_into` when error handling or retries
must recover the destination owner. Neither API imposes allocation or alignment
policy. Buffer contents must be initialized, as required by ordinary safe Rust
mutable byte slices.

The shared reader reads the XID, finds the pending operation, and parses only
the RPC envelope in small control storage. It then fills the caller's buffers
in order, directly from the stream, across any RPC record fragment boundaries.
`SegmentedReply` returns the original buffers, decoded envelope and total
`payload_len`. Only that many bytes across the buffers were received; remaining
bytes are unchanged. `AcceptedStatus::Success` in the returned envelope has an
empty payload. The actual XDR results live in the buffers, without a full RPC
assembly or payload decode copy.

A remote accepted error or rejected RPC retains its original envelope and has
no result bytes. The low-level API leaves application interpretation to its
caller, just as the existing raw transport API does. NFS response headers,
opaque-data lengths, padding and file-data ranges remain the NFS layer's job;
this interface does not infer a file-data boundary or require an NFS dependency.

## Pre-send admission and caller-controlled recovery

Use `connect_with_admission` or `from_stream_with_admission` to configure
`AdmissionLimits { max_calls, max_bytes }` for one physical connection.
`AdmissionLimits::default()` selects 1,024 accepted calls and 128 MiB of logical
admission charge. Existing constructors retain compatibility mode without
configured call/byte ceilings; enabling limits is explicit and does not add
fields to `ClientConfig` or change generated method signatures.

The limits apply together to classic and owned-receiver calls sharing that
connection. The byte charge covers the serialized request and marker, outgoing
segment descriptors and inline request/receiver state. It does not measure
process RSS, allocator rounding, shared backing capacity or receiver-owned heap
allocations. Applications remain responsible for their own backing-memory quota.
No full payload encode/coalescing is performed merely to decide admission.

`try_call_into(request, receiver, options)` makes a nonblocking admission decision
before allocating outgoing segment descriptors, registering an XID or writing
bytes. Success returns an owned future: polling it starts the operation and its
configured deadlines. Refusal returns `CallRefusal` with both original input
owners, a typed `AdmissionError` and `required_bytes`. `try_call` provides the
same contract for a classic `RpcMessage` (its receiver field is unit).

- `AtCapacity` is temporary: other calls currently hold the needed capacity.
- `TooLarge` is permanent for these limits, even when the connection is idle.
- `Closed`, invalid limits and invalid requests are distinct failures.

A refused request was not sent. The caller can keep it in an application-budgeted
scheduler, retry after `wait_for_capacity(required_bytes)`, select another
connection or close this one. Readiness is not a reservation: another caller
can obtain capacity first, so retry the admission decision. There is no hidden
retry loop or secondary queue of refused payloads. Congestion alone never closes
the connection. Existing convenience methods report admission failure through
their existing `RuntimeError` result; callers needing typed classification and
both owners should use the `try_` methods.

Each accepted operation holds a call slot and its byte charge through I/O cleanup
and consumption of its final outcome. A completed response still waiting in an
unpolled call future retains capacity too. Capacity is released before that
future returns its outcome. Dropping an unpolled future releases its reservation
without sending; abandoning a sent operation retains the reader's charge until
its destination is released. `admission_snapshot()` exposes the current counters.
Closing the connection wakes capacity waiters but does not pretend live owners
have already been released.

This contract distinguishes refusal from a timeout after sending. An interrupted
frame still retires the connection, and the owned-receiver reply deadline still
retires it to recover its destination. Detaching a timed-out receiver and draining
a late reply without closing the connection is separate future work. A failure
after sending does not prove that the remote procedure did not execute.

See the runnable [client](../examples/owned-buffers/src/bin/buffer-client.rs) and
[server](../examples/owned-buffers/src/bin/buffer-server.rs), which use generated
interfaces, refuse a second call, preserve both owners and retry after capacity
becomes available. [Run commands](../README.md#caller-owned-buffers) use two terminals.

## Limits and failure

`RecordLimits` bounds every incoming record, including its markers, and the
number of fragments. Length checks happen before reading an advertised body.
A caller-capacity failure drains the remainder using fixed scratch storage and
fails that call without treating the next record as payload. Malformed matched
reply envelopes, malformed framing, EOF, record-limit violations or interrupted
frame writes retire the connection and fail other outstanding calls. Unmatched
XIDs are drained within the framing limits without decoding their discarded
envelopes. An explicit `close` interrupts the connection; its reader retains any
active buffers until its task actually stops.

Cancellation before submission releases the caller's buffers immediately.
Cancellation while waiting for the writer removes the unsent registration.
Cancellation after starting a partial frame closes the connection so another
frame cannot follow a truncated one. Dropping a future after a complete send
abandons its result, but retains the outstanding XID and receiver until the response has been consumed or the
connection is closed. A late response cannot write into a buffer reused by
another work unit. A blocked, nonresponding peer can therefore retain memory
until connection retirement; the consumer remains responsible for a suitable
watchdog/retirement policy when dropping futures without a deadline.

A configured call or write deadline while queued for the writer removes only
that unsent registration and returns its receiver. Once frame writing has
started, a deadline retires the shared connection and awaits actual receiver
cleanup before returning the original receiver with the timeout error. This can
fail other calls sharing that generation, which must retry on another connection.
The configured deadline requests cleanup; returning the owned outcome can take
longer than that deadline while cleanup runs. To cancel explicitly and recover
the receiver, call `close()` and continue awaiting `call_into`; do not drop its
future. `close()` and `wait_closed()` alone report terminal state, not physical
quiescence. The owned outcome supplies the per-operation quiescence boundary.

The connection's reader and writer are independent of application scheduling. Existing
legacy calls and owned-buffer calls can be multiplexed together and correlated
by XID. Completed operations release their registration. Dropping the final
connection owner stops its weak reader. Ordinary kernel socket copies, and
backend-specific TLS record buffers, remain expected. This is elimination of
library payload staging, not a kernel-to-user zero-copy claim.

## Accounting and future backends

Payload memory is supplied by the caller. Control memory consists of the
fixed 432-byte receive envelope scratch plus at most 400 verifier bytes owned
by the decoded reply, an encoded request envelope (at most 840 wire bytes),
a 4-byte record marker, at most 64 stack slice descriptors on send, fixed 4 KiB
drain scratch on receive, per-connection task state and per-call registration,
oneshot and segment descriptors. Envelope wire lengths exclude allocator
capacity/rounding and descriptor overhead. Descriptor storage scales with the
number of segments, not payload bytes. Consumers must include those controls and live operation futures,
task/stack allowances and their underlying stream's requirements in admission
plans; the library does not enforce an application memory quota or admission policy.

The public operation transfers owned arguments and destinations and returns
owned results. That operation-level contract can be retained by a future
completion-based backend: it must retain owners through physical completion,
including cancellation. The current implementation uses ordinary `AsyncRead`
and `AsyncWrite`; no io_uring registration API is exposed speculatively.

## Relationship to oncrpc4j

This is a deliberate divergence in API shape, not in the ONC RPC wire protocol.
The oncrpc4j Grizzly pipeline decodes an XDR message before delivering a reply
callback. Its transport already supports composite/default sends and a raw TCP
segmented file-chunk send path. Rust's new path instead installs the destination
owner before receiving the reply and separates envelope parsing from result
placement. It expresses quiescence through ownership rather than assuming a
logical callback or timeout means every access has ended.

These responsibilities can serve as a model for a Java extension: an outstanding
XID owns its destination, a record cursor fills those destinations without an
RPC-sized assembly buffer, partial writes preserve frame integrity, and late
replies retain or discard their original owners. Reusing that model in Java is
separate work; the Rust change does not modify oncrpc4j.

## Validation and adoption

Deterministic record tests inspect actual source/destination pointers, fragment
boundaries, short reads and vectored writes, scalar fallback, EOF/errors and
limits. Transport tests exercise shared legacy/segmented calls, cancellation,
late replies, buffer identity and recovery after a capacity failure. Owned
receiver tests additionally exercise error and timeout return of the original
owner, reader callback failures/panics, explicit cancellation, admission under a
non-reading peer, completed-but-unpolled results, and a variable
application-prefix/data/padding receiver that appends short reads into one
allocation. Existing runtime, generator, server and TLS tests remain
compatibility checks.

The generator emits additive `*_into` and `*_segmented` methods and immediate
`try_*` variants. Metadata projections decode a statically bounded prefix and
leave a single terminal variable opaque body in caller storage. `FooSegments`
argument projections use `OpaqueSegments` to represent multiple retained owners
under one opaque length and padding. The ordinary typed methods keep their
existing representations and copying behavior. See the
[generated API contract](cli-spec.md#generated-caller-owned-client-api).

The public RFC 1813 NFSv3 READ/WRITE/COMMIT fixture exercises generated APIs with
optional attributes, typed error unions, fragmented records, short replies,
64-bit offsets and multi-owner WRITE data. This controlled-peer coverage does
not qualify interoperability with a deployed NFS server.
