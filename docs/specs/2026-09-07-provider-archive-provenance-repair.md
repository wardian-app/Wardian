# Provider archive provenance repair

Status: implemented locally; integration and native acceptance pending.
Issue: #1175. Depends on #1166; unblocks #1167 and #1169.

## Evidence and ownership

The archive boundary owns canonical narrative identity and replay. Adapters own
native source classification. This change starts from the Claude provenance
base and does not change Pi envelope mapping or Antigravity source/tool mapping.

Three observed failures require the shared repair:

- Antigravity corrections retain the same event ID, but append skipped known
  IDs before copying corrected provenance.
- Pi envelope IDs change the old provider-log hash. Old unrooted rows can mask
  the corrected event or coexist with it as a duplicate narrative.
- A retained fresh Antigravity run already stored a broker-generated input and
  its native database observation in one narrative's `event_refs`. Active
  text-based merge selected the generated row and hid `provider_log: true`.
  The native observation was present on disk; this was not a tool projection
  or assistant-commentary defect.

## Contract

A current event can enrich an archived observation only when Wardian agent,
provider, event kind, and native source/session binding agree, and its ID or a
verified alias intersects the stored identity set. Conflicting explicit native
sessions do not match. Conflicting non-null native roots or step-source evidence
fail with an error, before publishing that capture's repair. A capture cannot
write another agent's events into the requested archive.

Text equality and bounded-tail sequence numbers are not observation identity.
The Pi compatibility bridge uses a complete, session-headed native log snapshot,
whose filename contains the native session ID. It recomputes the exact former
hash and accepts an alias only when that hash maps to one observed native entry.
The adapter must already expose that entry ID. Repeated equal messages, missing
entry IDs, generic/reused paths, incomplete snapshots and bounded tails cannot
create this bridge. Once persisted, aliases work with a bounded tail and after
restart without rereading the original full log.

This bridge is scoped to Pi's pre-envelope-ID projection. It remains necessary
while such archives are readable; removal requires an explicit end to that
compatibility contract. There is no temporary sidecar, new event kind, bulk
migration command, or scheduled retirement mechanism.

The original event ID, narrative sequence, timestamp, source references and
unrelated metadata remain. Text and artifacts remain except for the verified
native tool completion described below. Verified IDs are added to existing
`legacy_event_ids` and narrative `event_refs`. Two already-stored aliases of the
same proven observation converge to one ordinary event and narrative record.
Repeated prompts with distinct identities remain distinct. Archive-only history
is retained. Missing original native evidence cannot invent a historical root.

Enrichment is finite: input origin/purpose, request root, causal reference,
context observation, provider turn and explicit step source. Proven native
source fields include `provider_log`, log/source paths, log source, provider
session IDs, step index, and raw type; the event's source is retained as native
source evidence. Missing fields and `context_observation: unreported` do not
erase stronger existing evidence. Presentation role follows the #1166 rules.
An older adapter can replay the original ID without reverting enrichment.

Already-reconciled delivered inputs use their persisted `event_refs`, not a
new text match, to expose native source metadata on the original generated row.
The broker's human/agent origin, request root and causal reference stay
canonical. A delivered row without a native observation cannot acquire
`provider_log`. Explicit provider context/internal events cannot reconcile with
a delivered prompt merely because their text is equal.

## Mutable native tool completion

A retained Antigravity capture exposed a second #1175 failure: SQLite step 2
(type 132, source 2) reached status 3 with the real scratch-file result, while
the same stable archived tool-result ID at narrative sequence 4 still held
status 2 and `Step is still running`. Its call projection was already correct.
This is stale shared archive state, not a new adapter mapping.

For the observed Antigravity SQLite GENERIC tool-result contract only, current
status 3 may replace archived status 2 text and `provider_step_status`. The
normal provider/source/session and event identity checks still apply; both
observations must agree on log source, native step type/source/index and tool
ordinal. Missing or conflicting location evidence cannot authorize mutation.
Status 3 proves completion, not success: the public outcome status is preserved.
A completed result cannot revert to running, and a different terminal result
without native revision evidence does not overwrite the first completion.

The event ID and narrative sequence, timestamp and source references survive.
The existing text materialization path stores large completed output and updates
its excerpt/artifact references. Retry can repair a stale narrative from an
already-published completed event. There is no extra event, alias ID, schema or
migration command. Enabled capture persists the repair; disabled live replay
repairs only the view. Historical rows without observed completion stay as-is.

## Publication and logging policy

Capture-enabled repair reuses `events.jsonl`, `conversation.jsonl`, sources,
materialized turns, manifest and index. Schemas are unchanged. Original IDs and
ordinary enriched rows remain readable by older archive readers.

Repair and append hold the existing per-agent archive mutex. Active and
standalone event/narrative/turn snapshot readers use the same mutex. Concurrent
current and older captures on the archive owner converge without losing known
provenance. This preserves the existing single runtime writer model; it does
not introduce or claim a cross-process, multi-file transaction.

Each affected file uses the existing atomic JSONL replacement. A failed
replacement preserves that file's previous snapshot and returns an error.
Events publish before the narrative; a subsequent capture also repairs a
narrative left behind after successful event publication. Derived turns,
manifest and index use the existing regeneration path. Duplicate capture also
checks derived snapshots so a prior failed turn/manifest publication is retried.
A failure between files can leave derived snapshots stale until retry; no original observation is
removed merely because source evidence is unavailable.

This recovery contract covers enrichment of already-linked archive records.
Fresh append has a separate, exact-base-reproduced failure window: an event can
be durable before its source/narrative reference, and retry can duplicate it.
[Issue #1183](https://github.com/wardian-app/Wardian/issues/1183) tracks that
pre-existing recovery gap. Existing-record repair tests do not claim to cover it.


Capture-disabled repair is a pure active-view projection. It can read the
matching open archive after its in-memory handle was discarded, but cannot
reopen a closed or differently bound conversation. It does not write archive
files or persist enrichment. The pre-existing disabled-capture cutoff behavior
is unchanged. Standalone replay can project an already-persisted delivered
identity link without writing. Other historical provenance remains unchanged
until an enabled capture observes the necessary native evidence. Re-enabled
capture can enrich already-logged identities despite the disabled cutoff;
that cutoff still prevents adding events seen only while logging was disabled.

## Verification

Isolated lower-layer checks compile the production archive modules, DTOs,
normalizer and identity helper without starting providers or the native app.
The retained Pi fixture supplies old/new boundary inputs from its real envelope
IDs; this is boundary verification, not a new Pi adapter acceptance claim.
The retained Antigravity fixture contains the actual generated/native pair and
its shared narrative reference. Explicit source-4/source-2 inputs are
supplementary boundary stimuli matching the adapter contract.

Baseline checks on the Claude base fail for the same-ID missing root, the Pi
missing envelope root, and the fresh Antigravity active merge (zero native human
rows instead of one). Fixed regressions cover those cases plus pre-existing Pi
double rows, restart/standalone replay, no-op recapture, older adapters, repeated
text, foreign sources/sessions, missing evidence, capture-disabled byte
preservation, concurrent captures, conflicting evidence, and Windows atomic
publication failure followed by retry. Existing archive/turn/normalizer checks
also pass in the isolated harness.

The retained running/completed tool fixture fails on commit `9972602a` at the
shared active projection. Fixed checks cover durable replay, original sequence
and identity, stale running capture, missing/foreign native locations, large
output materialization, no-op recapture and partial-publication retry.

Full application checks and local review are tracked with this implementation;
real-provider/native retest remains coordinator/QA acceptance work. No paid prompts or native app runs
are part of this implementation validation.
