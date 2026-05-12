# Spec 036: Agent Status Transition Gate

- **Status:** Implemented
- **Date:** 2026-05-12
- **Decider:** User

## Context

Agent status drives the roster, telemetry, Queue completions, and CLI watch/wait/ask behavior. Before this change, startup log replay and restored-agent hydration could look like fresh live work because old provider logs were parsed from the beginning. Some command paths also changed `current_status` directly and then emitted a status event separately, bypassing the backend duplicate suppression path.

GitHub issue #59 reported that agents still loading had unpredictable status. The required behavior is deterministic: startup and restored metadata enrichment must not replay old lifecycle transitions, and user-visible status events should be emitted once per real transition.

## Decision

Wardian treats provider log replay during startup as metadata enrichment only. The first telemetry parse of a discovered provider log may update metadata such as query count, start timestamp, and log path, but it does not record a status transition, update `last_status_at`, append a watch status event, or create Queue completion evidence.

Restored Codex and Claude log watchers begin at the current end of the provider log. They only consume new appended provider events after the restored runtime is active. Fresh live events still flow through the existing provider parser paths.

Frontend Queue flushing seeds status from the first metrics snapshot. It only treats later metrics changes as completion evidence, while direct live `agent-status-updated` events and buffered provider output still create completions for real turns.

## Consequences

- Restored agents no longer replay old `Processing... -> Idle` history as fresh completion evidence.
- Duplicate command-path status observations are routed through the backend status setter where practical.
- CLI `watch`, `wait --next`, `send --wait-until`, and `ask` continue to rely on live watch events created after their cursor, not stale startup state.
- Initial Queue state is deterministic and does not flush stale restored buffers from metrics alone.
