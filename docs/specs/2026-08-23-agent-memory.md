# Agent-owned memory

Status: Implemented
Tracking: [#727](https://github.com/wardian-app/Wardian/issues/727)

## Decision

Wardian provides provider-neutral long-term memory through a local SQLite
authority at `<WARDIAN_HOME>/memory.db`. Records are owned by one Wardian agent
and are either agent-wide or bound to a canonical workspace. Conversation
archives and provider-native histories remain independent evidence/runtime
systems; neither is the memory authority.

Direct retention and startup recall are opt-in while the feature matures. They
do not call a model when enabled. Automated consolidation is optional, disabled
by default, and implemented as an ordinary bundled workflow with ordinary
provider/model/effort assignments.

The global **Agent memory** setting in **Settings > Agent Runtime** defaults to
disabled. With it disabled, new interactive and headless provider processes
receive no memory instructions, startup brief, or managed memory capability;
existing records remain inspectable. The setting applies to future launches and
restarts, not an already-running provider process.

At the end of every user task, the provider independently checks whether the
user established a clear durable preference, project convention, decision,
correction, lesson, or current state. It saves that context in the same turn
even when the request is brief and does not use words such as "remember" or
"save". This is a required pre-final-answer lifecycle step, and the runtime
instruction includes the basic save/list/update syntax so the provider need not
load a separate skill first. Ambiguous and explicitly transient instructions
are not retained.

## Record lifecycle

Each logical memory has immutable revisions. Updating an active revision marks
it superseded and inserts its successor in one transaction. Removing memory
creates a retained tombstone state. Active recall excludes superseded, removed,
and integrity-invalid revisions. Stable memory has no expiry. Current state is
labeled stale after 30 days without verification but is not deleted.

Every revision owns normalized recall text, a durable evidence excerpt and
SHA-256 hash, and optional source links. Source links can open a conversation,
artifact, file, or workflow run for deeper inspection. Deleting either system
does not cascade into the other.

## Startup recall

Before provider launch, Wardian selects active agent-wide memory plus active
memory for the resolved workspace. A deterministic 12,000-character policy
balances stable memory with fresh current records, preferring a complete record
from both groups when they fit and alternating between them as space permits.
Selected records are presented stable first; stale current records have lower
priority. The policy reports omissions. A fingerprint covers scope, logical ID,
revision, kind, evidence integrity, text, staleness, and the budget-policy
version.

Fresh processes receive full `Stable memory` and `Current state` sections.
Restored/resumed processes compare against the latest fingerprint for their
provider process key and receive additions, revisions, removals, and stale-state
changes. When nothing changed, Wardian re-sends the active checkpoint because a
previous process may have exited before its model consumed the compiled brief.
Empty recall creates no synthetic turn. Storage/compile/audit failures fail open
for provider startup and are logged; save/update operations fail closed.

The brief and direct-retention contract are appended only to Wardian-generated
habitat instructions. Providers receive the generated context through their
normal instruction projection; Codex also receives it through a runtime
developer-instructions override because its real workspace remains the process
working directory. No user-authored instruction file is changed.

Every successful non-empty provider delivery records its own injection receipt,
even when the compiled checkpoint is unchanged. Headless delivery becomes
successful only after a zero exit; interactive delivery becomes successful only
after provider-ready evidence. A failed or pre-readiness launch records no
receipt, so its replacement receives the required context again. A repeated
fingerprint means the memory content is unchanged; it does not mean a later
provider process consumed an earlier delivery.

Fresh background workflow workers keep their synthetic provider-process
identity, but recall and managed memory commands use the registered agent named
by the assignment. This keeps workflow process isolation without creating a
second memory owner.

## Workflow boundary

`memory_commit` is the only workflow node that can mutate memory. It consumes a
strict `MemoryCommitBatch`, validates the complete request, and commits memory
operations, archive cursor movement, audit events, and an idempotency receipt in
one transaction. Replaying the same key and request returns the original result;
reusing a key for different content is rejected. Cursor movement is serialized
before mutations and must be strictly monotonic for its conversation stream, so
a late consolidator cannot apply stale operations or move the checkpoint back.
Wardian derives the cursor namespace from the authorized agent, normalized
workspace, and conversation ID; each conversation is a distinct epoch and the
executor binds those values, the source sequence, and idempotency key to the
trusted invocation boundary, so model-provided metadata cannot bypass ordering.
Direct save/update idempotency keys are globally unique and provenance is part
of the replay comparison.

The bundled `memory-consolidation` workflow is seeded like every other sample:
only when missing, never auto-run, and never overwritten after user edits. A
generic persisted session-close invoker can launch any workflow after a matching
conversation boundary. New invokers are disabled unless explicitly enabled.
Failures are workflow failures and do not roll back the agent lifecycle action.

Temporary-provider workflow assignments may optionally specify a model and
effort. This is normal workflow configuration, not memory-specific behavior.
Because those providers have no registered agent owner, they receive neither
durable-memory instructions nor a managed memory capability. A workflow must
assign a registered agent to read or save that agent's durable memory.

The engine renders the target `agent_id` into `memory_commit` only from the
canonical `&#123;&#123;trigger.output.agent_id&#125;&#125;` invocation value; model-produced
structured output cannot select the memory owner. Trigger input is not an
authority boundary. The executor separately requires an immutable authenticated
invocation principal supplied by the trusted session-close context or by a
managed CLI process whose launch-scoped memory capability the desktop validates.
It rejects unauthenticated commits and any rendered request or batch whose agent
identity differs from that principal. The principal persists with the run so an
approval resume cannot lose or replace the original authority.

Session-close workflow launch is a post-commit lifecycle effect. Wardian
captures the closing conversation while the old runtime is intact, starts the
replacement as pending, persists the proposed roster, and commits the archive
boundary before installing that replacement in the live map. Matching workflows
start only after the entire replacement commit succeeds. A failed step rolls
back durable metadata where necessary and leaves the original conversation
available to the next retry. A provider-start failure after the old process has
stopped leaves the registered agent in `Error` with its configuration and
boundary evidence intact. Every committed boundary receives a unique idempotency
component even when no archive is available. Concurrent invoker edits are
serialized under a filesystem lock so independent CLI and desktop writes cannot
overwrite one another.

Clear and resume also use a durable replacement journal before their first
roster mutation. The journal checkpoints proposed state persistence, SQLite
metadata persistence, boundary commit, and live installation under a
cross-process lock. Startup rolls pre-boundary phases back to the original agent
identity and post-boundary phases forward to the replacement identity before it
restores any provider. Offline memory lookup runs the same reconciliation and
fails closed while a live replacement still owns the lock.

The roster barrier fences ordinary `state.json` writers as well as replacement
and recovery code. Recovery uses an expected-config comparison and refuses to
overwrite a later legitimate update. Disabled-log boundaries persist their
capture cutoff during the journaled boundary commit. A replayable session-close intent
remains in the journal through live installation; startup dispatches it with a
deterministic per-invoker/per-boundary workflow run ID and removes the journal
only after dispatch is accepted.

Inside a Wardian-managed provider process, `wardian memory` authorizes only the
agent identified by `WARDIAN_SESSION_ID` and a matching runtime-issued
`WARDIAN_MEMORY_CAPABILITY`. Wardian stores only the capability hash and permits
concurrent provider processes for the same agent, so changing the claimed
session ID is not sufficient to impersonate another agent. Each process owns a
separate lease owned by its `ActiveAgent`; Wardian revokes it when terminating,
replacing, or reclaiming that runtime, not merely when a PTY reader fails.
Another agent's name or UUID is rejected for list, show, save, update, history,
and remove. The core store requires a `MemoryActor` for every operation and
constrains agent-actor SQL by owner; only the desktop host uses explicit
operator authority. The CLI has no absent-identity operator
fallback. Offline name lookup uses persisted roster state and must resolve to a
unique agent UUID.

## Observability

Memory save, update, remove, and non-empty load actions are stored independently
of provider logs and rendered as dedicated compact chat events. `Memory loaded`
expands to the exact injected context. Local and remote chat share the renderer.
Memory events are deliberately excluded from conversation archives.

Native GPT-5.6-Luna acceptance covers explicit save and isolation, fresh and
delta recall, implicit capture from ordinary tasks, correction/supersession,
agent-wide versus workspace scope, rejection of one-response-only formatting,
and later authoritative recall.

## Deferred

Local embeddings and semantic reranking, shared/cross-agent scopes, cascading
deletion, and memory-specific workflow reset/fork/restore controls.
