# Temporary agents: ownership, follow-up, and visibility

Status: accepted design with an implementation candidate; provider/runtime
acceptance remains pending.

## Purpose and decisions

Make automation workers and provider-spawned subagents inspectable and accountable
without turning each execution into a permanent agent profile. The primary caller
is an orchestrating agent or an automation executor, not a human launching another
terminal. Human surfaces expose useful oversight without changing their purpose.

Temporary workers need stable identity, owned lifecycle, follow-up where supported,
and complete activity attribution. They do not need a promotion-to-permanent
operation in the initial feature. Keeping a transcript is separate from keeping a
process running or retaining a resumable provider session.

Design work completed: current-view and execution-path inspection; scope and
ownership definition; lifecycle and visibility proposal; author-side failure-case
check; independent design review with zero blocking findings. Alternative view
redesigns are deferred because the existing agent and run surfaces already supply
the required inspection destinations. Implementation review and runtime acceptance
remain separate gates.

## Observed system at the design base

Source basis: `541d6b850022b7e3a902e25a89c5afac4c40d612`.

- `src/layout/watchlist/types.ts` defines agent and team entries. It has no run
  entries. `AgentWatchlist.tsx` renders those entries from agent configuration.
- `src/features/graph/graphProjection.ts` projects agent nodes, communication
  edges, and team/project/folder relationships. An automation run is not a node.
- `src/views/AgentsOverviewView.tsx` renders agent terminal/chat cards. Its
  headless-status handling does not make automation runs agent-list containers.
- `src/views/GardenView.tsx` separately obtains agent and automation projections.
- `src/features/automations/monitor/AutomationMonitor.tsx` is an existing run
  inspection surface, with its own run store and agent-scope filtering.
- `src-tauri/src/automation/mod.rs` and `resolve.rs` distinguish bound agents from
  ephemeral role/class/provider workers. Temporary execution already receives an
  automation-owned session ID and uses the runner. Extend this ownership rather
  than inventing another launch path.
- Built-in Codex subagents have separate provider rollout files. Observing a spawn
  tool call or a parent transcript does not establish ingestion of child activity.

These are source observations. Lifecycle controls for each provider still require
runtime verification; a provider's ability to write a log is not proof that Wardian
can interrupt, resume, or send follow-ups to that session.

## Agent experience

An orchestrator needs to ask the worker that produced a result for a clarification,
request a correction after review, or continue after a dependency finishes. These
are continuations of a bounded responsibility. Reuse is optional: a fresh worker
with a concise handoff can be preferable to carrying a large conversation forever.

The caller must be able to discover owned workers, inspect outcome and capabilities,
send a correlated follow-up, and release a worker when its responsibility ends.
Responses distinguish accepted delivery, completed work, unsupported operation,
expired resumability, and uncertain transport. No automatic retry of an uncertain
submission and no silent switch to a fresh conversation or composer injection.

Do not add a parallel messaging API. Extend the canonical messaging and inspection
contracts once provider-backed temporary identities can be addressed safely.
Record completion and blocked results durably so the owner can consume them later;
UI display and notification delivery must not be prerequisites for execution.

## Ownership and state

The Rust backend owns a persistent temporary-worker registry. Its records are
execution records, not ordinary reusable `AgentConfig` profiles. Frontend views
are projections; closing a card cannot terminate or delete a worker.

Each record requires:

- Stable `worker_id`, provider identity, and owning workspace.
- Origin: either a subagent's immediate parent and root agent, or an automation's
  definition, run, node, and attempt. A child of an automation worker retains the
  automation origin through its parent chain; do not fabricate a root roster agent.
- Separate provider session ID and runtime generation; neither replaces worker ID.
- Execution state, outcome, timestamps, last observation, and evidence provenance.
- Independent capability flags for inspection, follow-up, interruption, and resume.
- Transcript/telemetry source references, resource ownership, and retention state.

An automation attempt and a provider resume are different identities. Retries must
not overwrite a previous attempt's outcome or usage. Existing automation session
IDs remain compatible transport identifiers; assess run/node retry semantics
before changing their format.

Parentage is attribution, not a team, watchlist membership, communication edge,
or permission grant. A child may not widen its parent's execution authority.
Existing automation and messaging authorization remains authoritative. Validate
provider-reported ancestry against a trusted launch event or provider relationship;
workspace-path similarity and filenames alone cannot establish ownership.

## Lifecycle and recovery

Execution states are requested, running, waiting, succeeded, failed, cancelled,
and unknown. Provider-specific statuses map into these states with provenance.
An uncertain launch or lost connection becomes unknown, not failed or completed.
Unknown does not permit replay or resource cleanup that assumes process exit.

Automation worker registration assigns a runtime generation from the numeric
attempt. Running, unknown, and terminal transitions compare the worker id, owner
instance, runtime generation, and current active state. A late future from an old
owner or generation cannot overwrite a record already reconciled as unknown.
Pre-launch errors and observed non-zero exits are definite failures. Timeouts,
lost process observation, and post-submit response/parse failures are uncertain
outcomes and remain unknown.

Completed work may remain addressable for a follow-up while its provider process
is stopped. A follow-up creates another recorded invocation and reports whether
it continues a live session or resumes a retained one. Unsupported providers expose
inspection only; Wardian must not advertise controls that cannot work.

Retention is separate from execution: live resources, resumable session material,
and durable evidence have distinct ownership and expiry. A terminal worker is
eligible for provider resume for seven days after completion or the last explicitly
accepted follow-up. Detailed registry metadata and source references are retained
for 30 days from the same point. These are Wardian eligibility windows, not a
guarantee that a provider permits resume. They do not authorize deletion of
provider transcripts, conversation history, usage evidence, or user-owned
resources. Active and unknown workers remain available for reconciliation, and no
provider process stays alive solely to satisfy retention.

Releasing a worker is idempotent. Termination needs provider/owned-process
acknowledgement; retained workspace or provider-home resources cannot be deleted
while ownership is uncertain. Cancellation of a run propagates to its owned active
workers, records partial cancellation, and never kills unrelated sessions.
For an active automation-owned headless invocation, the cancellation marker is
observed by that invocation, its exact child process is terminated and reaped, and
the registry records a guarded cancelled outcome. The cancellation command reports
whether that worker acknowledgement was observed. This does not imply an interrupt
control for independently discovered provider children.

On application restart, reconcile persistent records with provider and process
evidence. Mark unresolved workers unknown and surface them through their owner.
Deleting or closing a parent view/profile does not discard child evidence or imply
that child execution stopped. Retain origin identifiers and an inspectable orphan
record when its original navigation destination no longer exists.

## Existing views

| Surface                | Proposed behavior                                                                                                                                                                               |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Watchlist              | Preserve agent/team entries. Temporary workers do not become top-level rows.                                                                                                                    |
| Graph                  | Preserve the agent communication graph. When temporary agents are shown, distinguish provenance from actual communication edges. Do not introduce automation-run nodes in this feature.         |
| Agent overview         | Preserve agent terminal/chat cards. Explicitly opening a worker uses a capability-appropriate inspection surface; a headless worker does not get a fictional terminal. No run-group containers. |
| Automation Monitor     | Run/step detail is the primary destination for an automation worker's status, output, usage, and supported controls, including its child workers.                                               |
| Garden                 | Extend the existing automation and agent projections with worker activity and links to canonical inspection; do not create a second run model.                                                  |
| Activity and telemetry | Show own and descendant usage separately, with a clearly labeled combined total. Automation workers belong to their originating run/attempt.                                                    |

A worker needing attention must be discoverable from the owner's existing surface
and the established attention mechanism even when its process is headless. A
provider child appears as a compact indicator on its root agent. For an automation
with no root roster agent, the destination is its Monitor run and Observe node
inspector. Do not add temporary watchlist rows or silently add workers to
user-maintained teams or lists.

The root-agent indicator opens the retained child records. It reports each child's
outcome, capability source, evidence/source coverage, own usage, and descendant
combined usage, plus a verified-descendant aggregate. Controls remain absent when
the canonical provider adapter has only observe capability.

Temporary-worker visibility must be explicit and stable: clearing search or changing
views cannot accidentally convert an inspection into permanent roster enrollment.
Acknowledging attention changes visibility, not the underlying failed outcome.

## Telemetry and records

Discover child transcripts through verified ancestry and source ownership. A shared
provider home must not cause every transcript to be attributed to every agent.
Preserve native source records and import each source once; parent/child aggregates
must not ingest or sum the same facts twice. Multi-level descendants count once in
the root total. An automation invocation on an existing persistent agent retains
that agent's identity and links its invocation to the run without creating a
duplicate temporary worker or duplicate facts.

Report coverage explicitly: complete for known sources, partial, or unavailable.
Unknown child usage is not zero. Keep input, cache-read, cache-write, output, model,
and effort dimensions distinct. A displayed token total is not an account bill.
Track a worker's waiting/completed state separately from active execution time.

Store canonical records through Wardian's existing database and archive mechanisms;
provide a readable inspection/export of identity, origin, lifecycle, and evidence
references. This spec does not introduce a second editable state file. Provider
logs remain evidence, not an interface for rewriting lifecycle state.

## Implementation sequence and acceptance

1. Add the typed run/node/attempt registry and verified ownership adapters for
   automation workers and provider children. Cover duplicate observations,
   retries, nested children,
   missing parents, restarts, and unknown launch outcomes at the backend layer.
2. Import child telemetry with provenance and deduplication. A real provider child
   with an independent transcript must contribute exactly once to its root/run;
   unrelated shared-home sessions must contribute nothing. Test partial coverage.
3. Expose inspection and supported canonical controls. Prove follow-up correlation,
   late completion retrieval, unsupported operations, expiry, and uncertain delivery
   without replay. Test release/cancellation acknowledgement and restart recovery.
4. Add owner-linked visibility in the existing surfaces. Browser tests can prove
   grouping and navigation but cannot qualify provider lifecycle behavior. Verify
   headless attention without requiring a terminal or permanent agent-list entry.
5. Run isolated native/provider acceptance for each supported adapter, including
   run cancellation and nested workers. Never discover or clean up by killing every
   matching provider process. No acceptance run may disturb an unrelated live app.

Existing agents, watchlists, teams, and runs keep their identities and behavior.
Historical child ancestry is backfilled only when supported by evidence; otherwise
retain unassigned evidence with partial coverage. The initial Codex adapter is
observe-first: it discovers raw rollout metadata and telemetry through verified
`parent_thread_id` ancestry. Follow-up, resume, and interruption remain unavailable
until a canonical adapter proves those capabilities for the recorded runtime
generation. Disabling the feature stops those new controls without deleting records
or abandoning ownership of already running work.

## Design review boundaries

The settled implementation uses structured registry fields rather than parsing
owner labels, seven-day resume eligibility, 30-day detailed registry retention,
owner-linked child indicators, and Monitor run/step attention. Provider control
capability proofs remain open. Permanent-agent promotion, run nodes in the graph,
run containers in the agent list, and a replacement automation engine are out of
scope. None is needed to deliver inspectable, attributable, bounded temporary
workers.
