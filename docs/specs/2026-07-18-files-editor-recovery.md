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
overwrite a newer buffer.

After restart, `get_file_recovery` may return only the stored base and buffer.
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
does not save.

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

Unreferenced hash blobs are collected only after a 24-hour grace period and
only when their filenames match the backend-owned blob pattern. This protects
readers from a recently interrupted or concurrent generation while bounding
long-lived crash debris conservatively.

A guarded save may include one exact recovery ID and expected recovery CAS
revision. After `saved` or `unchanged`, Wardian best-effort discards only that
generation under the calling WebView and resource scope. Another view's dirty
recovery is never scanned or deleted. Cleanup races and I/O failures leave the
recovery for later handling and do not turn an already committed file save
into a reported failure.

## Verification

Focused Rust tests cover create/update CAS, runtime recreation, scoped
read-only restore, discard CAS, manifest-last injected failure, immutable blob
generation selection, conservative orphan retention, oversized and tampered
bodies, path-escape rejection, stale clean merge, overlapping conflict
markers, revoked authorization, no current-byte read or write from recovery
alone, and exact cleanup after a successful guarded save.
