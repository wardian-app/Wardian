# Durable Files Editor Recovery

Date: 2026-07-18

## Decision

Wardian stores dirty ordinary-file editor buffers in a bounded backend-owned
recovery store. A recovery manifest is metadata, not filesystem authority. It
contains the stable `resource_key`, display metadata, base content hash, a
backend-generated opaque base revision, recovery CAS revision, originating
WebView label, and timestamps. It contains no subscription, picker grant,
retained handle, or private file revision token.

The store layout is:

```text
<WARDIAN_HOME>/files/recovery/<recovery-id>/manifest.json
<WARDIAN_HOME>/files/recovery/<recovery-id>/blobs/sha256-<content-hash>.txt
```

Base and buffer bodies are immutable content-addressed blobs. The manifest
names the exact base and buffer blob generation. Wardian writes and flushes
both blobs before atomically replacing `manifest.json` last. A crash may leave
a new unreferenced blob, but readers either observe the complete old manifest
generation or the complete new generation. Mutable `base.txt` or `buffer.txt`
aliases are deliberately not created because they could disagree with the
manifest after a crash and would duplicate stored bytes.

## Authority boundaries

Creating or updating a checkpoint requires the exact live file subscription
for the same canonical `resource_key`. A new checkpoint reads its base through
the retained authorized handle. Updates preserve that original base and use a
recovery compare-and-swap revision so an older debounce completion cannot
overwrite a newer buffer. `base_revision` and `base_content_hash` authorize
creation only. On update, the backend reauthorizes the recovery ID, exact
resource/WebView scope, recovery CAS generation, and current live subscription;
it does not require a restarted runtime to reproduce the private logical
revision that existed when the base was captured.

After restart, `list_file_recoveries` discovers body-free recovery metadata for
the exact stable resource key and calling WebView, newest first. Discovery
validates bounded ordinary blob metadata but does not synchronously read or
hash every body. The frontend then selects an ID and `get_file_recovery`
performs complete body validation before returning the stored base and buffer.
The backend scopes lookup to the exact stable resource key and the calling
Tauri WebView label. The label comes from `WebviewWindow::label()` and is never
accepted from request JSON. This read-only path does not open the current file,
write it, recreate a subscription, or revive an expired picker grant or
revoked agent root.

Reading the current disk head or merging requires a newly verified live
subscription for the same resource. `merge_file_recovery` refreshes current
authorized UTF-8 bytes and performs `diffy` three-way merge with:

```text
base = stored base
ours = stored editor buffer
theirs = current authorized disk head
```

The result is explicitly tagged `clean` or `conflicted`. Both variants include
current revision/hash metadata, whether disk changed from the recovery base,
and merged text. A conflicted result retains conflict markers and both sides;
the backend never silently selects either editor or disk bytes. Merge itself
does not save. The final clean or conflict-marker text is revalidated against
the complete Monaco model byte and line limits before it crosses IPC.

## Limits, integrity, and cleanup

Both stored bodies must be complete valid UTF-8 text within the centralized
Monaco byte and line limits. Reads revalidate blob size, UTF-8, line limits,
and the SHA-256 filename. Recovery IDs must be canonical UUIDs. Record and blob
directories must be ordinary direct children of their backend-owned parents;
manifests and blobs must be ordinary files. Manifest-controlled path traversal
and symlink-based reads fail closed.

Before invoking `diffy`, all three merge sides must also fit the centralized
per-side diff byte and line limits. A larger but still editable recovery remains
readable and discardable; only the resource-intensive merge is rejected.

Recovery writes and maintenance share one `recovery_io` critical section. The
store admits at most 128 recovery record directories and 512 MiB of ordinary
body files. Admission counts fresh manifestless records, malformed records,
live generations, and fresh unreferenced immutable bodies; it never evicts a
live or fresh record to make room. Capacity failures return
`recovery_capacity_exceeded` before publishing new bodies.

Every recovery-store sweep visits every direct canonical UUID record, not only
the record being opened. Unreferenced hash blobs and manifestless records are
collected only after a 24-hour grace period. Malformed records, unexpected
entries, live manifest generations, and fresh crash debris are retained
conservatively while still counting against admission where applicable.

A guarded save may include one exact recovery ID and expected recovery CAS
revision. After `saved` or `unchanged`, Wardian best-effort discards only that
generation under the calling WebView and resource scope. Another view's dirty
recovery is never scanned or deleted. Cleanup races and I/O failures leave the
recovery for later handling and do not turn an already committed file save
into a reported failure.

## Verification

Focused Rust tests cover create/update CAS, runtime recreation without retaining
a recovery ID, scoped discovery/restore, restart-safe recheckpointing, discard
CAS, initial and update manifest-last failures, immutable generation selection,
root-wide conservative sweeping, record/byte admission budgets, metadata-only
discovery, oversized and tampered bodies, path-escape rejection, final merged
model limits, stale clean merge, overlapping conflict markers, revoked
authorization, no current-byte read or write from recovery alone, and exact
cleanup after a successful guarded save.

## Residual filesystem race

Recovery path validation rejects preplaced symlinks, junction escapes,
non-files, and non-direct children before access. A hostile same-user process
could still swap a path between metadata validation and a later path-based
open/read on platforms where a portable no-follow retained-handle sequence is
not available here. Closing that TOCTOU window requires a separate
cross-platform handle-relative filesystem design and is not broadened into
this recovery lifecycle change.
