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
3. Keep that transition lease through provider startup readiness. Renew it while
   startup is pending; release it on provider `Idle`, terminal `Error`, or `Off`.
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
