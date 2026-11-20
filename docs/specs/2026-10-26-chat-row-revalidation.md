# Bounded Chat Row Revalidation

- **Status:** Accepted
- **Date:** 2026-10-26

## Context

A loaded Chat observation can lie outside both a new recent snapshot and the
next requested older page. A reset response cannot correct that observation
through page membership alone. A text detail response also lacks its complete
header, source coordinates, and body binding.

Source membership and display pairing answer different questions. A valid
observation can remain readable after an append while a previously unique
live/completed pair becomes ambiguous. The Rust backend owns both decisions.
The frontend maintains a bounded view over those records.

## Decision

Extend the existing Chat read boundary with a bounded metadata renewal mode.
Resolve backend-issued source observation IDs against the current admitted
source index. Return actual current projections using the existing
`AgentChatPage` envelope, with an explicit acknowledgement of the completed
request set.

This mode uses the existing source rows, member relations, read budgets, and
authorization boundary. It introduces no history scan, global alias ledger,
new endpoint, or detail-body expansion. Raw transcripts and the durable archive remain
canonical. Source heads and display relations remain rebuildable per-agent
derived state in the existing archive cache.

## Wire Contract

The existing Tauri command is `load_agent_chat_page`. Its optional Rust argument
is `revalidate_ids: Option<Vec<String>>`; the existing JavaScript command
argument convention exposes it as `revalidateIds`.

The existing HTTP endpoint is
`GET /remote/api/agents/{session_id}/chat`. Its optional `revalidate_ids` query
value contains one JSON array, encoded once through the normal URL query
encoder. Repeated query keys and comma-separated IDs are not alternate list
encodings. Existing remote authentication, origin checks, and audit behavior
continue to apply.

Both surfaces use the same producer and validation rules:

| Field | Renewal rule |
| --- | --- |
| `session_id` / `sessionId` | The active Wardian session or agent targeted by the read. |
| `revalidate_ids` / `revalidateIds` | A nonempty list of at most eight raw entries; each ID is at most 512 UTF-8 bytes. Validate raw limits before deduplication and source I/O. |
| `revision` | Echo the incoming page's opaque revision to bind the renewal cohort. |
| `cursor` / HTTP `before` | Omitted in renewal mode. |
| `detail_ref` / `detailRef` / HTTP `detail` | Omitted in renewal mode. |
| Response `revalidated_ids` | Optional or null until the complete deduplicated request set is authoritatively resolved. A successful acknowledgement accounts for every requested ID. |

IDs and cursors are opaque to callers. Select source IDs already issued in
loaded observation metadata or display-member references. A display ID can
represent multiple source observations. SDK message IDs, text equality,
caller-supplied coordinates, and filesystem paths cannot substitute for source
observation IDs.

For example, the renewal-specific command arguments are:

```ts
{
  sessionId,
  revision: incomingPage.revision,
  revalidateIds: knownSourceIds,
}
```

An ordinary canonical header can omit provisional admission metadata. Callers
must not manufacture such fields or require them as a substitute for an
explicit renewal acknowledgement.

### Ordinary scope and empty bootstrap

A canonical-only page stays on the ordinary cursor and refresh path when the
previous page, incoming page, and loaded window have the same nonempty session,
conversation, and generation identity; each page has its own nonempty opaque
revision; both page epochs are null; and neither page nor loaded rows carry
producer source IDs, source admission metadata, display relations, removals, or
renewal acknowledgements. A null epoch is valid for this canonical reader. It
does not prove a native source change, and it never authorizes native renewal.

If any loaded native evidence exists, an incomplete incoming native scope is
unknown even when the revision or reset flag is unchanged. Keep the last
coherent rows and older frontier and use the existing retry path. A positive
known session, conversation, generation, epoch, or admission mismatch remains
a scope change and follows the existing immediate clear/reload behavior.

Empty bootstrap is a separate guarded initialization relation for an empty page
with no canonical conversation head. Legacy `indexing` and
`projection_pending` states remain eligible under the same guards; a progress
label such as `ready` does not establish a canonical head by itself. A positive
conversation ID remains scope-bearing during legacy pending progress and is not
an empty bootstrap. The prior page and actual loaded window must contain no
committed rows, source/member evidence, admission, acknowledgement, aliases,
removals, or detail; no saved cursor, older demand, or staged native cohort may
exist, and the selected session must still match. A prior no-head page may
already carry positive generation and source-epoch hints. Preserve every known
hint: a positive mismatch is a scope change, while a dropped or partial hint is
unknown and retains the recoverable state.

The incoming page must contain usable rows and complete identity for its
evidence type: canonical identity with no native evidence, or a complete
current native scope. Canonical generated archive rows may coexist when their
generated ID matches the page conversation and they carry no native source
evidence. Each native observation needs its actual source ID/reference,
matching source epoch, and session. Ordinary header admission metadata is
optional on this first owned page; it is not an acknowledgement and cannot
authorize later native renewal. Strict current-scope admission and complete
acknowledgements remain required for metadata revalidation. An unchanged page,
empty acknowledgement, partial native scope, prior native cohort, prior
committed row, or older frontier cannot use bootstrap. Preserve the ordinary
request and cancellation guards while admitting the first complete page.

## Producer Invariants

1. Resolve IDs only within the active authorized source scope. Validate the
   exact stored observation, owner, source epoch, admission, source position,
   and allowed range. An ID can select an existing keyed row; its contents do
   not authorize the read.
2. Count all direct and related-row metadata reads against the existing object
   and byte budgets. Return current headers and aliases without fetching
   neighboring history or expanding bodies.
3. Publish an acknowledgement only when the entire bounded batch is valid
   against a complete current head. Recheck source ownership and head currency
   before publishing. Partial indexing, exhausted budgets, changed heads, or
   invalid observations cannot yield a partial successful acknowledgement.
4. A surviving unique pair uses its actual current physical completion and
   existing public display identity. Resolving a previously loaded live member
   alone must still consult the current relation and its physical partner.
5. When an additional observation retracts the pair, return each requested raw
   observation from its own stored row. Restore its own kind, role, turn
   fields, coordinates, source reference, and detail binding. Relabeling the
   completion as the live observation is invalid.
6. Withdrawing a display relation does not delete its physical observations.
   A still-present raw A must not be removed by the former pair's display
   tombstone. A missing or foreign ID is not evidence for an invented deletion.

Source coverage validation applies to admitted observations independently of
narrative eligibility. Valid tool calls and other recognized non-narrative
records must retain source validation while contributing no display-pair
cardinality. Eligible message provenance failures, malformed source coverage,
privacy changes, and incomplete framing remain disqualifying evidence.

## Consumer Transaction

Treat renewal as a bounded staged update to the currently loaded window.
A changed opaque revision while browsing older history requires renewal of
loaded source observations even when the incoming response has `reset=false`.
The reset flag alone does not certify unchanged display relations. Keep the
previous coherent window and its frontier while the cohort is pending.

Derive the required source and member IDs from that window and the incoming
page being admitted. A recent metadata poll does not add unseen recent rows
to the loaded membership. Divide the required IDs into requests of at most
eight and use one incoming revision for the cohort. Renew every required
member before committing the changed head or compatible reset; an
acknowledgement for A and B alone cannot authorize unrelated loaded rows.

Keep cohort responses separate from ordinary history pages. Renewal responses
do not advance the history cursor, switch browsing direction, or establish
that older history is exhausted. A failed or incomplete cohort cannot commit
half a reset. Resume through the existing bounded read and retry flow while
preserving request cancellation and fairness.

The temporary cohort holds at most two physical rows per selected source ID,
bounded by both the maximum loaded source-member set and the per-request record
budget. Each request still returns at most the backend's 512-record/1 MiB
limits. This finite staging allowance is separate from the visible 640-header/
2 MiB window: reconcile aliases and pair retractions first, then apply the
ordinary recent/older trim once at commit. A valid near-limit window must make
progress when one displayed pair expands into its two raw members.

Apply current semantic metadata from the renewed observation. Preserve the
loaded public display key and its position. Reuse an expanded body only when
its physical binding remains valid. Pair retraction must remove stale
completion metadata from raw A and admit the other already-loaded member as
its own raw row.

Cancel staged work after a selected-session or authoritative source-scope
change. Discard late responses from an earlier selection or window. A missing
acknowledgement provides no renewal authority; resolve retry and scope changes
through the existing normal read behavior. A provisional flag, matching epoch,
matching SDK ID, or equal text alone cannot authorize retention or pairing.

A same-scope head change, unknown scope, incomplete batch, or missing
acknowledgement leaves the last coherent rows and older frontier in place and
ends as a finite recoverable retry. Repeated polls for the same failed cohort
must not restart it continuously. The existing physical single-flight remains
held until each IPC/HTTP promise settles, even after the presentation deadline;
the phase limit prevents starting later batches but never unlocks an unsettled
read. A positive conversation, generation, epoch, admission, or privacy change
clears the old display immediately and follows the ordinary recent-read path.
User-facing renewal failures use the existing history retry presentation;
cohort and source-validation details remain diagnostics.

### Older frontier

After an older page is admitted, its `next_before` is the loaded window's
frontier. A compatible recent metadata poll while browsing older history
preserves that frontier. It does not replace it with the recent snapshot's
first older cursor or add unseen recent rows to the loaded membership.

`reset` reports invalidation and does not identify page direction. The optional
`older_page` response field defaults to false and is true only when the backend
honored the requested before frontier with an admitted source, canonical, or
bounded fallback older page. A rejected frontier that falls back to recent
headers, an unknown or changed scope, a detail read, and metadata renewal do
not set it. The consumer admits an older-demand reset as history only when this
field is true; it still applies the ordinary size, alias, removal, scope, and
complete-renewal guards before committing the returned cursor. Other resets
preserve the loaded cohort and its frontier.

Ordinary older requests use that actual saved cursor. An exhausted frontier
does not provide another ordinary older UI action. Tests that need a later
older demand must establish sufficient preexisting history before A, rather
than forge a cursor or append unrelated new rows to conceal a failed premise.

## Derived Cache Upgrade

Legacy source heads can persist a gap caused by treating a valid tool call as
an ineligible narrative. Completing a new writer's logic alone cannot repair
an already exhausted poisoned head.

The source-index implementation version invalidates that derived confidence.
An old head starts the existing bounded rewalk before its completion can be
certified again. Resetting the old gap bit is permitted only as part of this
rebuild. Real malformed, unowned, or unframed gaps must be rediscovered and
remain uncertified.

The transition preserves canonical source bytes, archive records, admission
boundaries, and stable observation identities. It requires no manual cache
clear or extra user command. Its compatibility scope is forward rebuilding
of legacy derived heads with one current producer owning a home. A downgrade
or mixed-producer write protocol requires separate compatibility evidence.

## Required Conformance

- H0 loads A through normal reads with a real non-exhausted older frontier.
  After exactly eighty distinct appended history observations, the actual H1
  recent page and the next ordinary older page both exclude A. Complete
  backend-issued renewal batches preserve A, its display key, and its binding.
- A same-scope source invalidation with a covered older frontier returns
  `reset=true` and `older_page=true` with the actual older rows and continuation.
  A rejected frontier that returns recent rows has `older_page=false`; the
  consumer neither prepends those rows to an older demand nor adopts its cursor.
- A later matching observation outside the incoming pages retracts the pair.
  Renewed raw A and B retain their own coordinates and full readable bodies.
  This repair also runs when the changed revision arrives with `reset=false`.
- Actual full producer DTOs and request sets exercise the shared consumer;
  fixtures do not add absent metadata or combine different source scopes.
- Valid non-narrative records permit complete source coverage. Foreign or
  malformed eligible observations still prevent certification.
- A legacy poisoned head repairs through ordinary bounded maintenance and
  reopen; a genuine source gap remains uncertified after rebuilding.
- Raw request limits, unknown IDs, partial batches, head changes, scope
  changes, stale replies, body-binding changes, and cursor neutrality are
  tested at their lowest meaningful boundary.
- The shared desktop IPC and remote HTTP paths carry actual captured producer
  DTOs and acknowledgements through the same staged consumer. Mechanical
  capacity tests cover split-pair trimming without changing visible or
  per-request limits.
- Genuine repeats, different native IDs, different phases, and different turns
  remain distinct under the existing display identity contract.

Targeted source or consumer checks establish their respective layers. Full
ordinary desktop and trusted HTTPS PWA history, paging, reopen, and body access
remain separate acceptance evidence.
