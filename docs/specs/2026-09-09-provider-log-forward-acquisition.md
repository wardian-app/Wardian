# Provider log forward acquisition

Status: implemented locally; authoritative backend and docs verification passed;
review pending.
Issue: #1251. Stacked on archive provenance repair #1189.

## Problem and ownership

The conversation archive previously projected an active provider JSONL log from
only its newest 2 MiB. A larger ordinary burst could retain a tool result and
assistant answer while omitting their earlier request or tool call. The source
still contained the relationship, but Wardian had no durable acquisition
cursor from which to read it.

The conversation archive owns acquisition progress. Provider normalizers own
the meaning of individual JSONL records. The persisted state is a private,
additive field in the agent's existing `conversation-capture.json`; it is not a
new public archive schema or source of user-authored truth.

## Source and continuity contract

Forward acquisition applies only to provider paths treated as append-only
JSONL. Antigravity conversation databases and OpenCode database projection keep
their existing readers.

Each source cursor binds all of these values:

- provider source key and canonical path;
- native identity obtained from the already-open file handle: volume serial
  and file index on Windows, device and inode on Unix;
- committed byte offset;
- a SHA-256 anchor over at most 4 KiB immediately before that offset; and
- bounded provider-normalizer continuation state.

The identity detects same-path replacement. File length detects truncation. The
anchor detects an in-place rewrite at or near the committed cursor. Acquisition
fails closed with no cursor progress when any check fails.

This is explicitly an append-only contract. A 4 KiB overlap does not prove that
an arbitrary older prefix was never rewritten in place. Wardian does not claim
that stronger guarantee.

## Batching and progress

One acquisition pass reads at most 256 KiB and commits only through its final
newline. A partial final record remains `pending` at the previous cursor. A
record that cannot fit within a pass, invalid UTF-8, invalid JSON, exhausted
normalizer state, replacement, truncation, or continuity mismatch is
`incomplete`; its reason is persisted and the cursor does not advance.

Status-triggered and restored-agent archive owners automatically request the
next pass while complete source bytes remain. Each pass releases the policy and
per-agent archive gates and yields before the next pass. UI-request paths may
complete one pass and rely on those existing owners for continued draining.

Normalization state carries request roots, provider turns, explicit tool IDs,
pending Codex context, and provider deduplication across batches and app
restarts. Count limits and a 1 MiB serialized-state limit prevent unbounded
private state. Exceeding a limit is an explicit incomplete state, not silent
loss or cursor advancement.

The archive's per-agent operation checks the expected prior cursor, performs
the ordinary archive append, and writes the next cursor only after append
success. Stale workers fail the compare-and-set. An append failure leaves the
old cursor retryable.

The archive still publishes several existing files. This change does not make
fresh multi-file publication transactional or repair every partial-publication
window. Issue #1183 remains the explicit boundary for that separate work.

## Logging policy boundaries

Global and per-agent conversation logging transitions record byte offsets on
the same source identity. A disable transition opens an excluded span at the
observed end of file. Re-enable closes that span at a later observed end.
Previously enabled backlog before the span can still drain; bytes inside the
span are skipped without normalization or archive storage.

Crossing an excluded or unknown interval resets pending normalization context.
Wardian therefore cannot attach disabled context to a later visible request.
Explicit tool identity present in a later provider record remains usable on its
own; no request root or provider turn is guessed across the opaque interval.

An existing source without trustworthy fresh-session evidence starts at its
current end with `unknown_before_offset` recorded. It is not silently
backfilled. A source created for a verified fresh provider session can start at
byte zero.

Privacy is fail-closed around setting persistence:

- disable observes the boundary before saving the disabled setting, so bytes
  racing the save can only be excluded conservatively;
- re-enable saves the setting before observing the closing boundary, so racing
  bytes can likewise only remain excluded; and
- an inaccessible or inconsistent active source prevents the policy mutation
  from claiming a boundary it could not establish.

## Concurrency

Callers snapshot the global agent roster before taking the asynchronous capture
policy gate. They then acquire only the archive's per-agent gate. Per-agent
configuration persistence releases its roster barrier before entering the
per-agent archive gate. This ordering prevents a policy transition from
deadlocking ordinary archive capture or agent replacement.

Policy generation and cursor compare-and-set reject a worker whose private
state was superseded. Archive retries remain idempotent through the existing
event identities and #1189 provenance reconciliation.

## Verification

The frozen #1189 base loses a leading Codex tool call from a source-equivalent
file larger than 2 MiB. The production regression drains bounded passes through
the ordinary archive owner and retains that call, its matching result, and the
final assistant answer.

Focused checks also cover opened-handle same-path replacement, truncation,
anchor mismatch, partial lines, serialized restart state, explicit tool
identity after restart, state exhaustion without progress, unknown prefixes,
disabled pending context, global and per-agent policy transitions,
append-before-cursor ordering, stale compare-and-set, and existing lifecycle
archive behavior. No provider is started and no paid request is sent by these
tests.
