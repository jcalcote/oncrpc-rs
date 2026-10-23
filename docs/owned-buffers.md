# Caller-owned RPC buffers

The optional `OwnedRpcTransport::call_segmented` API is additive in **0.2.3**.
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

The caller transfers a `Vec<B>` to the receive path, where
`B: AsMut<[u8]> + Send + 'static`. Buffer lengths represent writable capacity;
no allocation or alignment policy is imposed on `B`. A page-aligned CM buffer
can implement this interface without surrendering its allocation or reservation
owner. Buffer contents must be initialized, as required by ordinary safe Rust
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

## Limits and failure

`RecordLimits` bounds every incoming record, including its markers, and the
number of fragments. Length checks happen before reading an advertised body.
A caller-capacity failure drains the remainder using fixed scratch storage and
fails that call without treating the next record as payload. Malformed matched reply envelopes, malformed framing, EOF, record-limit
violations or interrupted frame writes retire the connection
and fail other outstanding calls. Unmatched XIDs are drained within the framing
limits without decoding their discarded envelopes. An explicit `close` interrupts the connection;
its reader retains any active buffers until its task actually stops.

Cancellation before submission releases the caller's buffers immediately.
Cancellation while waiting for the writer removes the unsent registration.
Cancellation after starting a partial frame closes the connection so another
frame cannot follow a truncated one. After a complete send, cancellation or a
call timeout abandons the result, but retains the outstanding XID and receive
buffers until the response has been consumed or the connection is closed. A late
response cannot write into a buffer reused by another work unit. A blocked,
nonresponding peer can therefore retain memory until connection retirement;
the consumer remains responsible for a suitable timeout/watchdog/retirement
policy. A returned timeout error is not a quiescence acknowledgement.

The connection's reader and writer share no CM scheduling policy. Existing
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
oneshot and segment descriptors. Envelope wire lengths exclude allocator capacity/rounding and descriptor
overhead. Descriptor storage scales with the number of segments, not payload
bytes. Consumers must include those controls, task/stack
allowances and their underlying stream's requirements in admission plans; this
library does not supply a process memory quota or a CM admission mechanism.

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
late replies, buffer identity and recovery after a capacity failure. Existing
runtime, generator, server and TLS tests remain compatibility checks.

NFS adapters and real NFS interoperability are separate integration work.
Generated typed XDR decoders still use their existing owned representations;
they do not automatically become zero-copy by using this release.
