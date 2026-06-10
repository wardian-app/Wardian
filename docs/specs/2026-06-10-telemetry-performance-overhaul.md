# Telemetry Performance Overhaul

Filename: `2026-06-10-telemetry-performance-overhaul.md`

- **Status:** Implemented
- **Date:** 2026-06-10

## Context and Problem Statement

With ~40 persisted agents, Wardian took minutes to start (with sustained disk
activity) and consumed 5-10% CPU while idle. The in-app telemetry meanwhile
reported far less CPU than the app actually used. Live profiling of a real
installation found:

- **~73 GB of disk reads in a 30-second idle window.** The gemini log
  discovery fallback walked the entire `~/.gemini/tmp` tree (365k files,
  2.4 GB on the profiled machine), fully reading and JSON-parsing every chat
  file, per gemini agent without a discovered log, on every 5-second
  telemetry tick. The gemini staleness check also re-read the whole
  discovered log every tick, with no mtime gating.
- **`sysinfo::System::refresh_all()` per tick cost up to 1.5 s** (live log:
  `sys_refresh_ms=1490`) because the telemetry pass harvested the command
  line and environment block of every process on the system — PEB reads on
  Windows — and `get_app_metrics` rebuilt the same marker strings a second
  time in the same tick.
- **Startup ran a full all-process environment scan per agent.**
  `cleanup_stale_persisted_session_processes` called
  `find_wardian_session_process_roots` (a `System::new_all()` +
  `refresh_all()` each) for every non-off agent, twice; with 24 such agents
  this alone accounted for roughly a minute of startup.
  `reconcile_headless_agents` then did another O(agents x processes x
  env-vars) nested scan.
- **Telemetry under-reported CPU by design of its failure mode.** Telemetry
  passes took 8-24 s against a 5-second interval (debug log: `Slow telemetry
  pass total_ms=24735 ... agent_count=40`), so subsequent ticks failed
  `try_lock` on the shared `System` and reported a literal 0 — precisely
  when the app was busiest.

## Proposed Decision

1. **Targeted process refresh.** The telemetry tick now refreshes only CPU
   and memory (`ProcessRefreshKind::nothing().with_cpu().with_memory()`).
   Command-line/environment markers, needed only for session-root discovery,
   are fetched with `UpdateKind::OnlyIfNotSet` (sysinfo caches them per
   process) on discovery passes gated by a 60 s TTL or a change in the agent
   set, and the discovered roots are cached in `SESSION_ROOTS_CACHE`.
2. **`get_app_metrics` reuses the cached session roots** instead of
   rebuilding marker strings for every process, and returns the last known
   sample instead of zeros when sampling is contended.
3. **Gemini I/O gating.** The staleness re-read only happens when the log's
   mtime differs from the last parsed mtime; the `~/.gemini/tmp` fallback
   scan retries at most once per 60 s per agent when nothing matched.
4. **Batched startup scans.** A new
   `find_wardian_session_process_roots_for_sessions` resolves the roots for
   all sessions with a single system scan, and the per-call scan uses a
   targeted refresh (cmd + environ only) instead of `System::new_all()`.
   `reconcile_headless_agents` indexes `WARDIAN_SESSION_ID` values in one
   pass.
5. **Debug log rotation.** `wardian_debug.log` rotates to `.log.old` at
   16 MB; it previously grew without bound (35 MB on the profiled machine).

## Consequences

- **Positive**: Idle disk traffic drops from ~2.4 GB/s to near zero;
  telemetry ticks complete in milliseconds instead of seconds, so reported
  CPU/memory stay current and the 0-CPU contention artifact disappears.
- **Positive**: Startup no longer performs ~50 full all-process environment
  scans; stale-process cleanup is one scan (two on retry) regardless of
  agent count.
- **Negative**: Newly spawned provider subtrees that are only identifiable
  via environment markers may take up to 60 s to be attributed to an agent's
  telemetry (the primary PID tree is still tracked immediately, and the
  cache refreshes early when the agent set changes).
- **Negative**: A gemini session whose log appears between fallback scans is
  discovered up to 60 s late.

## Follow-ups (not in this change)

- Incremental JSONL parsing: provider log parsing still re-reads the whole
  file when its mtime changes; large active transcripts could track a byte
  offset instead.
- Startup restore still spawns all non-off providers sequentially; bounded
  concurrency would shorten cold start further.
