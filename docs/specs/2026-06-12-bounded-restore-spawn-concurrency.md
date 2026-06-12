# Bounded Restore Spawn Concurrency

Filename: `2026-06-12-bounded-restore-spawn-concurrency.md`

- **Status:** Implemented
- **Date:** 2026-06-12

## Context and Problem Statement

On startup Wardian restores the persisted agent roster in two passes: pass 1
publishes every agent immediately (headless agents final, PTY agents as inert
"Restoring" placeholders), pass 2 spawns the PTY providers and replaces each
placeholder as it becomes ready.

Pass 2 ran **sequentially**. Each spawn costs seconds — stale-process cleanup
scan (Windows), PTY open, provider CLI boot, readiness wait — so with a
25-agent roster the tail of the queue sat on its grey pulsing "Restoring"
placeholder for more than five minutes while every earlier provider launched
first.

## Decision

Spawn restored PTY agents through a `tokio::task::JoinSet` gated by a
`Semaphore` with `RESTORE_SPAWN_CONCURRENCY = 4` permits.

- Placeholders still publish the full roster instantly (pass 1 unchanged), and
  each task replaces its own placeholder and emits `agents-updated` as it
  completes, so agents stream in as they become ready rather than in strict
  roster order.
- Four concurrent spawns keeps worst-case restore time roughly `ceil(n/4) ×
  spawn-cost` without thundering-herding the provider CLIs (node-based CLI
  boots are CPU-heavy) or hammering SQLite/process-scan paths.
- Failure handling is unchanged: a failed spawn records `Error` status and
  replaces the placeholder with an explanatory inert agent.

## Consequences

- **Positive**: A 25-agent roster restores in roughly a quarter of the wall
  time; no agent waits behind the entire rest of the queue.
- **Negative**: Provider launches now overlap, so any hidden cross-session
  assumption in a provider spawn path (shared file locks, login caches) would
  surface as a restore-time race. Per-session habitat directories and
  per-session settings files make this unlikely; the live multi-agent restore
  is the validation gate.
