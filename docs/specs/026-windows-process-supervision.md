# 026 Windows Process Supervision

## Status

Accepted

## Context

Wardian launches long-lived interactive provider sessions through `portable-pty` on Windows. Earlier cleanup relied on explicit UI actions, `ActiveAgent::drop`, startup process scans, and per-agent Job Objects assigned after `spawn_command` returned a PID.

That post-spawn assignment leaves a failure window. If Wardian crashes before the assignment completes, if assignment fails, or if the provider has already created console host descendants, Windows can leave provider or console-host processes running outside Wardian's ownership.

## Decision

Wardian initializes an app-lifetime Windows Job Object during startup and assigns the Wardian backend process to it before restoring or spawning agent sessions. The job uses `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so provider processes inherit crash cleanup from process creation time. The static job handle is intentionally kept for the process lifetime and is closed by the OS when Wardian exits.

Per-agent process-tree termination remains the normal cleanup path for kill, pause, resume, and clear actions. Per-agent Job Objects are now a fallback only when app-level supervision could not be installed.

Startup also sweeps stale persisted interactive sessions before restore. The sweep skips off agents and database-marked headless runs, then kills process trees discovered through Wardian session command-line markers or `WARDIAN_SESSION_ID` environment markers.

## Consequences

- Crashes and force-closes clean up newly spawned interactive provider trees more reliably.
- Normal UI termination remains fast and explicit.
- If Windows refuses to assign Wardian to an app-level job, Wardian logs the failure and falls back to per-agent job assignment plus startup stale-process cleanup.
- Native runtime E2E remains the required layer for proving real ConPTY and process-tree behavior.
