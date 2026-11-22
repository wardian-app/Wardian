# Explicit archive sequence vacancies

Status: accepted. Scope: operator repair of a proven never-written narrative
slot, tracked by #1796.

## Authority and representation

A conversation owns an append-only `vacancies.jsonl` beside its canonical
files. It is separate from `conversation.jsonl`, `events.jsonl`, `sources.jsonl`
and `turns.jsonl`. Existing coalescing and retired-slot records keep their
original meaning. A vacancy asserts absence and records explicit approval; it
does not assert that lost content was recovered.

Each LF-terminated line is one JSON object:

```json
{
  "schema": 1,
  "kind": "sequence_vacancy",
  "conversation_id": "<conversation-id>",
  "sequence": 3,
  "evidence": {
    "committed_head_sequence": 5,
    "prev_record_hash": "<sha256>",
    "next_record_hash": "<sha256>",
    "scan_digest": "<sha256>"
  },
  "authorization_id": "<authorization-uuid>",
  "approval_reference": "<recorded-approval-reference>",
  "approval_text": "<recorded-approval-text>",
  "created_at": "<rfc3339>"
}
```

The ledger holds at most 64 records. An encoded line, including its LF, is at
most 4 KiB. Unknown fields, malformed or unterminated lines, duplicate sequence
numbers and duplicate authorization IDs are rejected. No loader drops a torn
tail or repairs the ledger automatically.

The sequence must satisfy `1 < N < committed_head_sequence`. The committed
head is the highest persisted physical narrative sequence, rather than the
manifest's record count, an event coordinate or a projection watermark. Both
`N-1` and `N+1` must be intact physical narrative records. First-slot and
adjacent multi-slot holes are outside this initial operator command.
Before writing a new approval, reject any pending generated envelope at or below
that proposed physical head. A proposed ledger must already satisfy its own load
boundary; a failed authorization must not leave a newly invalid ledger behind.

## Evidence and conflicts

The scan reads only the conversation directory. It never opens provider
locators or external paths. Links, unsupported schemas, malformed structural
evidence and unresolved ownership prevent a proof of absence. All current
conversation files participate in the evidence inventory.

No narrative, generated event, owned body, artifact or structural reference may
occupy `N`. Exact turn endpoints, compact-ref endpoint aliases, request and
assistant-result sequences, failure evidence and side-effect evidence at `N`
disqualify it. An intact range with `seq_start < N < seq_end` alone does not.
Recognized generated IDs, legacy and provider-observation identities, and
`provider_mirror_request_root_ids` are resolved according to their archive
ownership. Malformed mirror-alias arrays and unresolved recognized identities
reject the proof. Arbitrary numeric suffixes and content integers are not
sequence references. General event positions, provider cursors, byte offsets
and native row IDs retain their own coordinate systems.

Schema-1 generated events written before recovery envelopes existed may omit
`archive_record` only when one physical narrative owner resolves their exact
`generated:<conversation-id>:<sequence>` identity. This matches normal recovery's
existing owned-row rule. Such an event cannot reconstruct a missing narrative,
provide a physical neighbour or head, or bypass any reference check. A present
but malformed envelope remains invalid. Existing files are never upgraded or
rewritten to manufacture that evidence.

Recovery and prefix validation accept a vacancy only after rechecking its
identity, bounds, physical neighbours, direct closures and absence of evidence
at `N`. A later real record at `N`, changed neighbour body, malformed ledger or
invalid closure is a conflict. Loading fails closed with a named rule and
operator guidance. Capture persists the terminal diagnostic through the
existing capture-error and `agent show` surfaces. Neither record wins silently.

An existing vacancy's evidence authority ends at its recorded committed head.
Authorization-time absence proof remains strict. During load, only two later
event-first publication windows defer to ordinary recovery: a pending generated
record with an exact archival sequence strictly above that boundary, or a typed
unowned native narrative event physically after the historical owned-event
cohort. Pending rows never establish the physical head or a neighbour. A pending
row at or below the boundary, unresolved historical data, or any reference to
the vacant slot still rejects validation.

The load classifier uses the same pure source factories as normal publication;
missing generated sources are reconstructible only through
`generated_sources_from_record`. It creates no recovery plan, persisted marker
or approval. Native recovery retains its existing record factory and reserves
validated vacancy slots. All explicit references and aliases are scanned, and
physical neighbour closures are rechecked even during a deferred tail window.
Recovered publication preserves the exact physical JSON-line bytes of unchanged
narrative rows. It rejects a changed or removed protected neighbour before
publishing sources or narrative; unrelated rows retain their normal update rules.
This preflight also covers capture refreshes with zero recovered rows and direct
event rewrites. One validated snapshot spans the serialized publication, so load
validation does not interrupt a normal body/event/source/narrative write window.
An unchanged replay of a protected owner publishes no enrichment or duplicate
body. Full text matching its one existing owned body through the same contained
SHA-256 binding retains that already published representation; other event fields
still have to match. New aliases, sources or changed protected fields reject before
their publication. No persisted marker or new operator authority is introduced.

## Hash framing

All hashes are SHA-256, encoded as lowercase hexadecimal. `U64(x)` is an
unsigned eight-byte big-endian integer. `F(bytes)` is `U64(length) || bytes`.
Tags and paths are UTF-8. A physical JSON-line payload excludes its line
terminator and otherwise retains its exact bytes.

The neighbour digest hashes this concatenation:

1. `F("wardian.vacancy.neighbour.v1")`.
2. `F("narrative") || U64(1) || F(physical narrative payload)`.
3. `F("events") || U64(count)` followed by `F(digest)` for each unique
   directly referenced event record's raw 32-byte payload digest, sorted by
   digest bytes. Recognized aliases resolve to their physical event record.
4. `F("sources") || U64(count)` followed by the equivalently sorted unique
   raw 32-byte digests of directly referenced physical source records.
5. `F("bodies") || U64(count)` followed by `F(relative path) || F(digest)`
   for each directly owned body, sorted by normalized relative path. The
   digest is the raw 32-byte hash of the complete file. Paths bind bodies to
   their references, so swapping two bodies also changes the closure.

The closure follows only the neighbour's own direct references. It does not
recursively traverse event or source relationships. Referenced rows and owned
bodies must exist and resolve safely within the same conversation.

The scan digest hashes `F("wardian.vacancy.scan.v1") || U64(file count)`, then
`F(relative path) || U64(file length) || F(raw 32-byte content digest)` for each
conversation file in sorted relative-path order. It records the inventory at
authorization time. It is audit evidence, never an equality requirement on a
future archive. Normal appends may change that inventory while preserving the
vacancy's current absence and neighbour closures.

## Operator command and persistence

`wardian agent archive-vacancy <agent> --conversation-id <id> --sequence N
--dry-run` reports the evidence, eligibility and named rule outcomes. It writes
nothing. An exact agent UUID supports an isolated home containing only that
conversation copy. A dry run grants no authority.

Writing requires `--authorization-id`, `--approval-reference` and
`--approval-text`. The desktop revalidates those fields and the current scan
under the per-agent archive lock. No offline mutation fallback exists. A retry
with the identical authorization, sequence and approval returns the existing
record. Reusing an authorization or an occupied sequence for a different
request fails.

Logical append preserves every prior ledger byte. The writer builds the
verbatim prior bytes plus one new LF-terminated line. It uses the existing
long-path-safe `NamedTempFile` atomic-replace helper: sync the temporary file,
atomically persist it over the destination, then sync the parent directory on
Unix. After replacement, while still holding the archive lock, reread and
verify the old prefix byte for byte and the complete expected result. A
failure is reported loudly without truncation, rollback or automatic repair.
Failure before replacement leaves the old ledger intact.

After a validated append, the matching missing-slot diagnostic can be cleared
and derived Chat state invalidated. A retired adoption remains retired. The
response states whether normal `adopt-archive` approval must be re-granted;
the vacancy itself grants no native capture or provider interval.

## Presentation and compatibility

Vacancies render nothing and contribute no narrative, event, turn, step or
artifact count. Turn totals use physical members or existing persisted logical
contributions. Pagination traverses stored keys; it does not infer membership
from `seq_end - seq_start + 1`.

Older builds retain their gap guard and cannot use this ledger as authority.
Operators rebuild before writing a vacancy. Canonical narrative and existing
coalescing formats are unchanged. This repair never deletes, renumbers,
silently skips or manufactures a narrative record.

## Required proof

Tests cover read-only dry run, each validation failure, later real-record
conflict, changed and swapped neighbour bodies, ordinary future appends,
malformed and torn ledger tails, failure before replacement, preservation of
prior line bytes, idempotent approval, readers and physical totals across a
vacancy, exact neighbour bytes across recovery, both later event-first recovery
windows, rejection of historical pending rows and aliases to the vacant slot,
and stopped adoption recovery through normal
re-grant admission. Unix directory enumeration uses target-specific `libc`
bindings, with vacancy tests on both Intel and Apple Silicon macOS CI lanes.

The reported live candidate is assessed only on an isolated private copy.
Published evidence contains sequence numbers, rule names, pass/fail outcomes
and hashes. The copy is deleted after the dry run. The real repair remains an
operator action after merge, rebuild and the user's approval.
