# Guarded Atomic Text Saves

Date: 2026-07-17

## Decision

Wardian saves editable text through the existing backend-owned file-resource
subscription. A save request carries `resource_id`, `subscription_id`, the
frontend revision number, the editor buffer's base content hash, and submitted
UTF-8 text. Only the exact live subscription is resolved. The resource entry's
non-serializable `FileRevisionToken` and retained `AuthorizedPath` remain the
write authority and never cross IPC.

`resource_id`, frontend revision, content hash, canonical path, and prior
snapshots are identifiers or optimistic-concurrency inputs. None is filesystem
authority by itself.

## Retained-handle save invariant

The core write primitive:

1. validates the submitted text against the centralized complete-model byte and
   line limits;
2. binds the private revision token and base hash to the retained authorization;
3. locks and scans that retained handle to prove the expected bytes are still
   current;
4. stages a unique sibling file, copies applicable permissions, writes all
   bytes, and flushes the staged file;
5. re-resolves every live subscription's current backend-owned agent or user
   claim;
6. scans the expected size and hash a second time through the same locked
   retained capability, then revalidates the original pathname, canonical root,
   and file identity;
7. atomically replaces the same canonical target; and
8. returns a newly authorized retained handle plus a newly scanned opaque
   revision token.

On Windows every retained authorization explicitly shares read, write, and
delete access. Existing targets use `ReplaceFileW`; the save-call handle is
released only after the final binding check while the authorization mutex stays
locked. On other platforms the sibling rename is atomic within the parent
directory. Byte-identical submitted text is an explicit `unchanged` result and
does not replace the file.

The final retained-handle scan rejects same-identity writes that complete while
Wardian is staging or revalidating authority. Ordinary filesystem replacement
does not provide a cross-process compare-and-swap primitive: an uncoordinated
external writer that begins after the final scan can still race the atomic
replace. Wardian's guarantee is therefore optimistic conflict detection through
the last retained-capability scan, not exclusion of external writers after that
boundary.

Save, watcher refresh, explicit refresh work, and subscription cleanup use one
per-resource operation mutex. Admission to an already-open resource uses that
same mutex, reauthorizes after acquiring it, and retries if the entry incarnation
changed. The exact subscriber membership is stable from candidate capture
through commit-time claim validation and handle rebinding. A successful save
installs its new private token and descriptor, rebinds every prevalidated live
subscription to the new file identity while preserving each subscription's
requested path and authorized root, increments the frontend revision once, and
emits one revision event. A watcher echo of those same bytes refreshes metadata
without creating a second logical revision.

## Optimistic conflicts

The IPC result is tagged as `saved`, `unchanged`, or `stale_conflict`. Every
variant returns only the current frontend revision and content hash. If an
external same-identity write invalidates the private token between the entry
check and the core write, the runtime refreshes through the still-authorized
subscription and returns metadata for the resulting current revision. It never
returns file bytes as conflict metadata.

The save command does not pass an agent-configuration snapshot as authority.
The runtime resolves each claim from its backend-owned resolver initially and
again through the core's pre-replacement callback. Revoked agent roots, revoked
exact-file grants, changed subscriber membership, replaced file identities, and
retargeted symlink or junction paths fail closed before replacement or event
publication.

## Exact-target Save As

The native save dialog mints a backend-only, 60-second, one-shot grant. The
grant retains:

- the opened identity of the selected parent directory;
- the parent spelling and verified canonical parent;
- the exact selected basename; and
- either an existing ordinary target's retained authorization and private
  revision, or a requirement that the target remain absent.

Before consuming the target grant or touching the destination, Wardian acquires
an owned reservation in the bounded ordinary-file capability table. A saturated
table with no inactive eviction candidate returns `grant_limit_reached` without
consuming the target grant or mutating either a missing or existing destination.
The reservation is held through filesystem commit, making subsequent capability
publication infallible. Once capacity is reserved, use removes the target grant
before filesystem work, so later success and failure both consume it. Parent
identity or path retargeting, a newly created absent target, an existing target
identity change, symlinks, directories, and sibling names are unauthorized.

For an absent target, Wardian writes and flushes a sibling staging file, then
uses an atomic no-replace commit: a hard-link commit on Unix-like systems and
`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` on Windows. For an existing
target, it uses the retained-handle guarded replace primitive. The result is a
new ordinary exact-file capability and `file:<canonical-path>` resource
identity.

Save As accepts no source resource or artifact identifier. It therefore cannot
retarget or close the current session. Opening the returned ordinary resource
and closing a source remains a separate frontend transaction.

## Verification

Focused Rust tests cover stale revision and base hash, revoked roots, replaced
identities, symlink and junction retargeting, permission-preserving atomic
replacement, explicit unchanged results, watcher echo suppression, exact
one-shot Save As, target and parent binding races, post-stage same-identity
mutation, concurrent subscription admission, commit-time claim revocation,
saturated missing/existing Save As destinations, and preservation of the open
source resource.
