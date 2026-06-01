# Provider Status Transition Consistency

- **Status:** Implemented
- **Date:** 2026-05-31
- **Decider:** User

## Context

Wardian status labels drive roster state, CLI wait/watch behavior, mailbox
delivery, and action-needed alerts. Real provider CLIs expose status evidence
through different live surfaces: structured stream JSON, transcript JSONL,
permission hooks, and terminal prompts. The status label must prefer the earliest
explicit action-required or completion evidence without allowing generic activity
to erase an unresolved action-needed state.

## Decision

Wardian applies provider PTY stream status events as live status evidence for
providers whose terminal output is the earliest source of truth. For Claude and
Codex stream events, generic `Generating` and `UserQuery` activity does not
overwrite an existing `Action Needed` status. Explicit `ActionRequired` still
moves the agent to `Action Needed`, and explicit `ModelResponse` or
`TurnCompleted` clears it to `Idle`.

Telemetry log parsing is also allowed to clear a stale Claude `Action Needed`
status when the transcript later shows a definitive idle state. Initial log
replay remains metadata-only and does not emit status transitions.

Wardian approval actions are delivered to live PTYs when the target is currently
`Action Needed`. Provider-specific approval keys are used for known interactive
prompts, so Codex and Antigravity accept actions submit the highlighted default
with Enter instead of queueing a literal approval message.

Antigravity workspace trust and permission prompts are parsed from terminal text
as `ActionRequired` events. This lets the roster and CLI move to `Action Needed`
while the trust gate is visible, before any JSON transcript evidence exists.

## Consequences

- Claude, Codex, and Antigravity action-needed labels can update from the live
  stream path instead of waiting for a later log or hook observation.
- Generic progress or generation events no longer hide an unresolved
  action-needed state.
- If the live watcher misses the clearing transition, telemetry can still
  recover from stale action-needed once the transcript records completion.
- CLI approval actions can unblock real provider prompts and move the agent back
  through `Processing` toward `Idle`.
- Antigravity trust prompts are treated as first-class action-needed gates.
- Existing duplicate suppression in `set_agent_status` continues to collapse
  repeated observations from PTY, hook, and transcript sources.
