# Agent memory architecture

The authority is `crates/wardian-core/src/memory.rs`. It opens the Wardian-home
`memory.db` with WAL, foreign keys, a busy timeout, and transactional lifecycle
operations. The CLI and Tauri commands call the same store.

Provider-integrated memory is gated by the global `memory_enabled` app setting,
which defaults to false while the feature matures. When it is disabled, startup
recall, direct-retention instructions, managed capabilities, and
`memory_commit` execution are off; the desktop operator path may still inspect
existing records.

Startup integration lives in `src-tauri/src/manager/spawn.rs`; generated
instruction projection lives in `src-tauri/src/utils/fs.rs`. Codex uses the same
generated text through a runtime `developer_instructions` config override so
Wardian does not modify the real workspace's `AGENTS.md`. Provider code must not
select or persist memory. Keep startup compilation deterministic and free of
model calls.

Keep the direct-retention instruction self-contained. It must include basic
save, list, and update command syntax and identify the retention check as a
required pre-final-answer step. Do not make reliable default retention depend on
the provider first locating or reading the Wardian CLI skill.

Headless execution has two identities when a fresh automation worker represents a
registered agent: `wardian_session_id` remains the synthetic provider-process
key, while `memory_agent_id` is the registered owner used for recall, injection
auditing, and the child process's `WARDIAN_SESSION_ID`. Direct headless delivery
uses the same real agent UUID for both. Never use a synthetic automation ID as a
memory owner.

Memory chat events are projected in `src-tauri/src/commands/chat.rs` and rendered
by the shared transcript row. Do not archive them as conversation records: their
source and retention lifecycle is `memory.db`.

Automation mutation is routed through the `memory_commit` engine node. Its payload
is a `MemoryCommitBatch`; the live executor calls `MemoryStore::commit_batch`.
The store acquires the SQLite writer lock and advances a strictly increasing
conversation cursor before applying mutations. A stale batch therefore rolls
back without writing memories, events, a cursor, or an idempotency receipt.
The store derives that cursor namespace from the authorized agent, normalized
workspace, and conversation ID. The executor binds workspace, conversation,
sequence, and idempotency key to trusted invocation values before opening the
store; model-produced cursor metadata is not authority, and a new conversation
starts a distinct cursor epoch.
`MemoryCommitRequest.agent_id` is rendered from the canonical
`&#123;&#123;trigger.output.agent_id&#125;&#125;` invocation value by the engine. No other template
is accepted. That rendered value is not authority: the live executor also
requires an immutable invocation principal supplied by a trusted session-close
launch or by a managed CLI process whose launch-scoped capability the desktop
validated. The principal is persisted in `invocation.json` for approval resume.
The executor must reject unauthenticated commits and any request or batch whose
`agent_id` differs from that principal before opening the memory store.
Do not add implicit memory writes to task, shell, script, state, or notification
nodes. Generic session-close invokers are persisted under
`library/session-close-invokers.json` and launch the ordinary run path.
Use their generic `require_archive` option for automations whose inputs only make
sense when a durable closing archive exists; a skipped run consumes no provider
quota.

Mutate session-close invokers only through
`wardian_core::session_close::mutate_invokers`; it holds the adjacent `.lock`
file across reload, mutation, and atomic replacement. Mutations report an error
for malformed or unreadable existing bytes and leave the file untouched; a
missing file remains an empty fresh-install state. A session-close context
owns a unique `boundary_id`. Lifecycle code may capture the closing transcript
before replacement. It then starts the replacement as pending, persists the
proposed roster, commits the archive boundary, and only then installs the new
runtime. Matching automations launch after that commit. A failed step must keep
the prior registered agent record and captured conversation available for
retry. If provider startup fails after Wardian has stopped the old process, the
agent enters `Error`; retrying Clear reuses the preserved boundary evidence.
Before either lifecycle path mutates `state.json`, SQLite, or the archive, it
writes a cross-process-locked replacement journal. Phase checkpoints let startup
recover the original identity before boundary commit or roll the replacement
forward after boundary commit. Offline `wardian memory` resolution performs the
same recovery and refuses to read the roster while a live replacement owns the
journal lock.
All `state.json` writers share that barrier, and recovery applies only when the
current config still matches the journal's original or replacement fingerprint.
Disabled logging writes its capture cutoff during journaled boundary commit. The journal
also retains the session-close intent; restart replay uses a deterministic run
ID per invoker and boundary before the journal is acknowledged.

The CLI treats a non-empty `WARDIAN_SESSION_ID` as a managed principal. Managed
callers must also present a matching `WARDIAN_MEMORY_CAPABILITY`, which the
runtime issues per provider process and stores only as a SHA-256 hash in
`memory.db`. Concurrent processes may hold capabilities for the same agent.
Each `ActiveAgent` retains its exact capability lease and revokes it when
Wardian terminates, replaces, or drops that runtime. The PTY reader must not own
a revoker: reader/broker failure is not proof that the provider child exited.
Revoking one process does not invalidate another concurrent process.
Managed callers may resolve and mutate only themselves; changing an environment
variable alone does not authorize another identity. Keep persisted `state.db` resolution
available when the desktop control endpoint is offline, but reject unknown or
ambiguous names rather than treating them as memory owner IDs.
Memory reads and mutations accept a full memory ID or a unique prefix. Resolution
is centralized in `MemoryStore` and scopes candidates to the authenticated actor;
exact IDs take precedence, a unique prefix resolves to its canonical full ID, and
an ambiguous prefix returns a distinct error without disclosing candidate IDs.
Mutation events, revisions, and automation-batch results always persist and return
the canonical full ID. Batch idempotency hashes are computed from that canonical
representation, including historical IDs needed to replay an inactive batch.
Unknown or cross-agent IDs retain the managed CLI's redacted access-denied
response.
The CLI has no operator fallback: absence of `WARDIAN_SESSION_ID` or its matching
capability fails closed. Cross-agent user administration belongs to the desktop
host's explicit operator path.

Every `MemoryStore` read, write, recall, audit, and commit operation requires a
`MemoryActor`. Agent actors are constrained by `agent_id` in the underlying SQL;
the desktop host uses the explicit `Operator` actor for user-directed
cross-agent administration. Do not restore ID-only store methods or rely on a
CLI-only ownership check.

Save and update idempotency keys are globally unique. Replays must match the
complete normalized request, including source provenance; another agent or a
different source set receives a validation error rather than a partial replay.

Headless processes receive durable memory only when Wardian supplies a
registered `memory_agent_id`. Temporary provider workers keep their synthetic
process identity but receive no memory instructions or capability, preventing
orphaned synthetic-agent records.

When changing selection or rendering, increment the memory budget-policy version
so resumed providers recover with a full changed fingerprint. An unchanged
resume still receives the active checkpoint: an injection audit does not prove
the prior process submitted a provider turn. Preserve these tests: revision
lifecycle, agent/workspace isolation, integrity exclusion, fresh/resume delta,
bounded rendering, batch atomicity/idempotency, provider instruction delivery,
compact event disclosure, and live temporary-agent recall.

Do not deduplicate injection rows by fingerprint. Fingerprints describe memory
content; injection IDs are delivery receipts. A successful non-empty headless
run records after its zero exit, while an interactive runtime records only after
provider-ready evidence. Failed or pre-readiness launches must not advance the
checkpoint. OpenCode uses its provider-owned ready title as that evidence because
its raw PTY stream has no stable compose marker. Every accepted delivery appends
a distinct row and `loaded` event.
