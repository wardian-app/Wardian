# Agent Restore Process Ownership

Status: accepted
Date: 2026-09-23
Scope: Cross-process provider startup and recovery on Windows, macOS, and Linux

## Context

An inherited `WARDIAN_SESSION_ID` identifies a Wardian context, not the owner of
the process that inherited it. A tool descendant can be a Python server, shell,
Node program, or another CLI. The old startup path treated the marker as both
headless evidence and authority to `taskkill /T`; this could suppress restoration
for an orphan or terminate an unrelated descendant. Persisted `Headless` status
can also outlive its execution lease. Conversely, one startup lease snapshot
cannot exclude a renewal or acquisition that happens later in roster restore.

## Decision

1. Treat only an unexpired `background_resume` or `background_fresh` lease as
   evidence that a cross-process headless execution is active. Ignore persisted
   `Headless` status as an ownership signal. Clear persisted PID when publishing
   `Off` or `Headless` without a verified PID.
2. Immediately before every interactive provider spawn, acquire a persisted
   `lifecycle_transition` lease through the shared conversation lease store.
   Its locked read and acquisition are the final exclusion boundary against a
   concurrently renewed or newly acquired background lease.
3. Keep that transition lease through provider startup readiness and publication.
   Resume and clear transfer their exact `agent_lifecycle` acquisition to spawn
   after terminating the old owned runtime; the lifecycle heartbeat stops and
   the spawn watcher takes over renewal without releasing and reacquiring.
   Under the cross-process file lock, the inherited path retargets and renews
   that exact acquisition to the selected provider session, using
   `fresh_provider_session_id` when `resume_session` is empty. It rejects another
   active owner of the agent or the new session. The same acquisition excludes
   the agent throughout a fresh session rotation. The inherited path still
   scans ambiguous provider candidates at the spawn boundary.
   Before stopping an owned runtime for clear or a fresh resume, persist a
   separate, exact-owner hold on its previous nonempty provider session. The
   hold conflicts by that session only, does not imply headless execution, and
   survives retarget and restart. If the previous and new identities match,
   withhold spawn. Child-handle exit and a successful tree-kill request do not
   prove that every provider writer exited across supported operating systems;
   keep the old-session hold until verified repair.
   For providers whose native-owner stop does no work, create the hold only at
   the synchronous runtime-detach boundary after fallible archive and identity
   preparation. A failed preflight leaves the original runtime and session
   unchanged. Codex creates the hold under the roster lock immediately before
   native-owner capture, after fallible stop preflight. Once capture begins,
   an error or cancellation retains the hold because provider exit is uncertain.
   If a non-Codex clear aborts after detach but before any stop, release the
   exact hold only after restoring the same runtime incarnation and handles.
   Once `spawn_command` succeeds, the spawn lease also retains on error or
   cancellation during setup. Readiness explicitly releases its exact owner.
   Release on provider `Idle`, terminal `Error`, or `Off` only after the exact
   runtime generation and status incarnation are published.
   The mock provider has no later prompt signal, so its parsed `Init` signals
   readiness only after caller-owned session identity validation returns
   `Confirmed`. Release waits for publication of that exact runtime generation
   and is fenced to the exact lease acquisition. A rejected or uncertain `Init`
   does not signal bootstrap. Before publication, even a terminal status cannot
   establish that a live provider is absent; retain exclusion for inspection.
   If replacement spawn returns but resume or clear commit fails, its attempted
   process stop does not prove the provider exited. After the stop attempt,
   the caller signals the watcher to stop renewal and leave the exact lease
   persisted until its finite expiry. The failed command reports `Error` and
   explains that retry waits for expiry and inspection of any provider candidate.
   The retry still acquires a new lease and scans provider candidates before spawn.
   Resume, clear, and startup restore own a publication disposition from spawn
   through roster commit. Dropping that disposition on async cancellation marks
   the unpublished runtime failed, stops watcher renewal, and keeps the exact
   lease only until its existing finite expiry. Commit disarms it immediately
   after the matching runtime enters the roster. Cancellation may leave the
   existing roster placeholder until startup recovery or a later lifecycle
   operation; it does not imply provider readiness or confirmed exit.
   Other providers retain their readiness rule after publication.
   New-agent registration also keeps its spawn lease while roster publication
   and any provisional Codex attachment are pending. A matching `Idle` before
   registration commits cannot release it. Failure or cancellation stops the
   watcher renewal and retains the lease until its finite expiry; exact runtime
   publication and successful registration are both required for release.
4. Scan the current process table for the configured provider invocation and an
   exact Wardian session association. Treat a match only as a possible provider
   candidate. A candidate blocks automatic restore and reports its PID for
   inspection. It never authorizes process termination.
5. Do not automatically kill processes during persisted-session recovery or
   pre-spawn cleanup. Lease expiry does not establish that a live provider is
   stale. Ambiguous candidates remain untouched until a person verifies and
   resolves them.
6. A marked non-provider descendant, including `python -m http.server`, does
   not block restore. Provider-name text outside an executable invocation,
   including `cmd /c echo codex` and `node script.js codex`, is not a candidate.

## Alternatives considered

- **Trust the environment marker and kill its process tree:** rejected because
  descendants inherit the marker and tree termination can reach unrelated
  servers or tool processes.
- **Use the executable name and marker as proof of ownership:** rejected because
  an agent tool can launch the same provider CLI under the inherited marker.
- **Use only the startup lease snapshot:** rejected because it can be stale by
  the time a provider is created. The spawn reservation rereads and acquires
  under the cross-process lease lock instead.
- **Kill when the lease expires:** rejected because expiry is not evidence that
  a process exited or stopped using its provider conversation.

## Consequences

- An orphan marked Python server neither blocks restore nor enters an automatic
  termination path.
- A live provider candidate with no active lease may leave the agent in `Error`
  with the candidate PID and a withheld restore. Inspect its executable,
  command line, and parent; stop it through its owning Wardian instance or
  normal OS controls only after confirming its role, then restart Wardian.
- The temporary transition lease protects cooperating Wardian processes that
  share the same Wardian home through provider readiness. It does not create a
  durable launch identity for arbitrary descendants.
- An uncertain previous-provider hold does not expire automatically. To repair
  it, stop all Wardian instances using the home, verify that the old provider
  session has no live writer, back up `runtime/conversation-leases.json`, remove
  only the hold entry with the verified `owner_kind`, `owner_id`, and
  `acquisition_id`, then restart. If exit cannot be verified, keep the hold.
- Process argument and environment visibility differs by OS and permissions.
  The process scan is best-effort candidate detection, not complete process
  identity proof. The lease store is authoritative only for cooperating
  Wardian operations that use it.

## Evidence and assumptions

- `crates/wardian-core/src/conversation_lease.rs` defines the execution lease
  modes and cross-process file lock.
- `src-tauri/src/manager/spawn.rs` is the interactive provider creation
  boundary; provider readiness is observed through runtime status.
- `src-tauri/src/utils/process.rs` matches executable positions and the
  session association while preserving process argument boundaries.
- Process-table candidates are assumed live at observation time. No inference
  from a missing lease or stale database PID can prove otherwise.

## Related decisions

- [Agent Lifecycle Locking](./2026-05-21-agent-lifecycle-locking.md)
- Wardian issue #1398
