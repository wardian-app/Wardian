# Agent Lifecycle Locking

* **Status:** Accepted
* **Date:** 2026-05-21

## Context and Problem Statement

Wardian exposes agent lifecycle operations through both the desktop app and the CLI. Operations such as kill, pause, resume, clear, and workflow headless handoff can touch the same agent runtime, PTY handles, input senders, state persistence, and SQLite metadata.

Holding the global agent map lock while terminating processes, spawning providers, bootstrapping provider sessions, or writing state can make unrelated app and CLI requests wait behind slow runtime or filesystem work. Moving slow work outside that lock improves responsiveness, but it also creates commit windows where two lifecycle operations for the same agent can interleave.

## Decision

Wardian serializes same-agent lifecycle operations with a per-session lifecycle mutex stored in application state. Lifecycle commands acquire the mutex for the target session before changing runtime ownership. This preserves cross-agent concurrency while preventing same-agent operations from racing.

Lifecycle handlers follow this lock policy:

1. Hold the global agent map lock only long enough to detach runtime handles, mutate visible in-memory state, update input sender ownership, and capture an ordered state snapshot.
2. Release the global lock before process termination, provider bootstrap, provider spawn, SQLite writes, filesystem cleanup, state-file writes, and frontend emits.
3. Commit replacement runtimes and their input sender together while the global lock is held.
4. Terminate displaced runtimes after the replacement commit is visible.
5. Persist `state.json` from an immutable `AgentConfig` snapshot captured under lock, then written after lock release.

## Consequences

* **Positive:** Kill, pause, clear, resume, and workflow headless transitions no longer hold the global agent lock across process termination or provider startup.
* **Positive:** CLI and app lifecycle calls for the same agent cannot overwrite each other's runtime or input sender state.
* **Positive:** State-file writes no longer require the global agent lock to remain held during synchronous filesystem work in the touched lifecycle paths.
* **Tradeoff:** The lifecycle lock registry retains one small mutex entry per session ID touched during an app run. The registry is reset when the app restarts.
* **Tradeoff:** Persistence remains best-effort; failures to write `state.json` or SQLite metadata are logged or ignored consistently with existing behavior.

