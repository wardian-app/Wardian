# Durable Agent Spawn

- **Status:** Source handoff; verification pending
- **Date:** 2026-11-10
- **Issue:** #1710
- **Scope:** Durable CLI spawn admission, operation observation, launch ownership, and recovery.

## Problem

The CLI's previous spawn request waited for the provider registration call to
finish. If the control exchange timed out after the app accepted the request,
the caller could not distinguish a lost reply from a rejected spawn. Retrying
could create a second agent. Startup recovery also had no durable record to
separate an interrupted launch from a saved agent that was safe to restore.

## Decision

Keep the existing `AgentSpawn` wire request for compatibility and add
`AgentSpawnAsync`, operation lookup, and bounded operation wait requests. The
CLI uses the new protocol for both synchronous and `--async` spawn. The normal
command still renders the registered agent using its existing field-selection
options. An older app's rejection is reported as `unsupported_version`; the
CLI never falls back to the old request after an uncertain exchange.

The CLI generates or validates a UUID operation key before sending. A generated
key is durably written to a private, minimal receipt before the request reaches
the app. The canonical typed spawn request is hashed and bound to that key.
Exact retries return the original operation and planned Wardian UUID; changing
the request under the same key conflicts. Observer settings such as `--async`
and wait timeout do not affect the request hash.

A queued exact retry schedules the same fenced operation after a transient
claim failure. One observer deadline covers the database lookup and current
identity evidence, including lifecycle and roster contention. Expiration leaves
the worker running and returns a timeout or unavailable observation.

## Durable ledger and admission

`state.db` owns the operation ledger and name claims. Admission uses one short
immediate transaction to write the canonical request, payload hash, planned
Wardian identity, any manual provider identity, resolved name claim, timestamps,
and `queued` state. It checks exact-key deduplication before enforcing capacity.
The initial bounds are two active workers and 64 queued operations.

OpenCode and Antigravity may discover their first native session after
registration. A separate immutable receipt records that first binding after
the watcher proves its current config and runtime generation. An exact saved
restore can admit the pending first discovery; a fresh-session replacement
cannot inherit the original operation's unassigned identity.

Queued intent is not a saved roster entry and does not appear as a fake live
agent. A worker claims the operation with an attempt UUID, prepares the config,
then changes the state to `launching` immediately before entering bootstrap or
registration. Preparation and provider work run outside database and roster
locks. Provider-derived identity is recorded before subsequent launch steps.

The operation states are:

- `queued`: admitted, not yet claimed by a worker;
- `preparing`: a worker owns the attempt, with no provider start authorized;
- `launching`: the provider bootstrap or registration boundary may have begun;
- `registered`: the exact planned incarnation was published and any required
  Codex attachment completed;
- `failed`: a pre-launch validation failure was established;
- `outcome_unknown`: launch or cleanup could not be proven; replay is disabled.

`registered` records historical success. Restore creates a new runtime from the
normal saved roster and never reconstructs one from the operation ledger. It
does not assert prompt, authentication, model, or provider-input readiness.

## Name ownership and startup recovery

Name arbitration uses owner-checked durable claims across async admission,
legacy spawn, rename, clone, final registration, and removal. Claims remain held
through uncertain launch outcomes. Exact operation retries do not allocate a
second name or identity.

Legacy spawn, clone, and rename also retain an exact attempt-to-agent
association with purpose, previous/new name, and a pre-effect phase. The adapter
reserves the name and inserts that association in one transaction, before effects.
records completed publication only after the actual JSON roster and required
completion agree under the current-incarnation guard. Ownership finalization
is an idempotent transaction. Rename promotion and previous owned-name release
commit together. A post-publication storage failure returns an agent/attempt
correlated ownership-pending disposition and preserves the published agent.

The exclusive-owner startup hook reconciles completed attempts against both
the actual saved roster and SQLite. This repair changes ownership metadata.
Provider readiness and uncertain operation outcomes retain their existing
evidence requirements. Missing/mixed publication or completion evidence keeps
the attempt bound and excludes restore. Definitely unstarted attempts can
release only their own reservation after checking the durable stores.

Removal selects ordinary compensation or a pending legacy attempt once, before
changing JSON. Ordinary removal keeps the exact stopped live incarnation through
the SQL result. On failure, fresh lifecycle ownership and Arc/generation checks
precede compensation from the current live roster. A changed owner prevents
compensation and deletion.

Only the absolute latest eligible pending legacy association selects bound
removal. Its existing attempt ID, sequence, and agent ID form the deletion ticket.
Private nullable `removal_state` and typed `removal_binding_json` bind the actual
database and saved-config identity, runtime generation, filesystem profile
identity, covered claims, and joined closure receipt. No new public protocol or
deletion worker is introduced. A CAS intent precedes JSON removal. Once absent,
the roster is never reconstructed for that ticket.

After owned process closure, a separate short transaction seals `closure_proved`.
The final transaction checks the latest ticket (including terminal attempts),
actual JSON absence, database/profile identity, fresh ownership, and covered
claims before deleting the agent and marking the ticket `done`. Foreign claims,
uncertain operation claims, and attempts outside the closure coverage retain
their owner links. A DELETE abort retains the sealed proof, row, claims, and
profile. Intent without sealed closure remains pending, even if the config says
Off or an old PID is absent.

The existing delete entry accepts the exact agent UUID after live removal.
A new lifecycle owner can finish only sealed metadata, with the same checks and
no replacement owner. A done receipt makes a lost reply idempotent. Exclusive-home
startup runs this repair before ordinary name finalization and restore. Present
or mixed JSON, changed profile/database ownership, unsealed closure, or uncovered
uncertainty permits no automatic deletion, provider launch, or roster replay.

After the app wins exclusive home and endpoint ownership, it converts
interrupted `preparing` and `launching` operations to `outcome_unknown`, then
queues never-started work before control service startup and saved-roster
restore. Both the initial restore publication and the actual restore spawn
check the ledger. Queued, preparing, launching, and unknown planned Wardian
IDs are excluded. A registered ledger row does not create a roster entry if the
saved roster is absent or incompatible.

## Codex launch authority

The worker selects current Codex policy and provider source during `preparing`,
so changes made while work waits in the queue apply to that launch. It captures
the Wardian home, effective sandbox/approval/trust policy, canonical upstream
home and explicit/unset selection, plus non-secret fingerprints for shell
policy, upstream `config.toml`, `auth.json`, `cap_sid`, and selected API-key
environment values. Only hashes and source descriptors are stored in the
private operation row; credential contents are not persisted.

The ephemeral launch context retains the bounded config and credential bytes
from the same reads that produced those fingerprints. Projection uses those
bytes. Descriptor serialization and debug output omit their contents. Malformed
config or auth documents fail during preparation. Each external start still
revalidates the current source and policy before proceeding.

The selected policy and upstream path flow through bootstrap arguments,
projection, shared Codex owner startup, and TUI launch. Each external start
revalidates the selected policy and source fingerprints immediately before the
OS start. A changed, malformed, or unreadable source fails closed. Projection
reads the selected source directly and cannot reload a different `CODEX_HOME`
or replace a read error with permissive defaults.

The authorization boundary is the final successful revalidation before the OS
starts the process. Wardian locks do not control external writers, so a change
after that boundary applies to future launches rather than revoking a process
that has already started.

## Bootstrap process ownership

Codex session bootstrap retains the owned child and drains stdout and stderr
concurrently with bounded retained output. One monotonic 60-second work budget
covers parsing, stream EOF, and child completion. A completion marker alone
does not complete bootstrap. Cleanup has a separate bounded budget and records
the observed child PID, exit status, stream EOF, and containment result in the
private operation ledger. Observer disconnect does not cancel the worker.

Each executable Codex fallback has an exclusively created attempt home. Its
configuration and credential leaves come from that attempt's selected context;
workers never share a workspace bootstrap projection.
After verified process closure, each attempt's config and credential leaves are
retired; uncertain cleanup retains them for inspection. Windows retains a
kill-on-close Job Object through stream closure and child exit. Executable
bootstrap fails before process start on platforms without retained containment.
The local Codex rollout path remains available on those platforms.

Bootstrap tests use controlled local fixtures and do not launch a provider or
use account credentials.

## Downgrade and operational limits

The additive schema is not downgrade-safe while unresolved operation-owned
claims or planned identities remain. An older app does not honor those claims
or restore exclusions and may collide with or restore an interrupted launch.
Resolve or manually reconcile queued, launching, and outcome-unknown operations
before downgrading. No automatic replay or roster reconstruction is permitted.

This work does not change Chat, Serve, trust authorization, introduce a daemon
or HTTP endpoint, or deploy/replace a live application. Full build and test
verification, independent local review, issue-linked publication, and hosted
CI remain later delivery gates.
