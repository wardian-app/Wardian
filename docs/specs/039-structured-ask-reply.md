# Spec 039: Structured Ask Reply

- **Status:** Implemented
- **Date:** 2026-05-13
- **Decider:** Tan Gemicioglu

## Context and Problem Statement

`wardian ask --until output:<token>` is useful for compatibility, but it is fragile for Wardian-managed agents because provider terminal output can include submitted prompt text, status bars, wrapped TUI repaint content, or retained screen text. That makes terminal-output matching a poor default completion signal for agent-to-agent requests.

Wardian needs a backend-owned request/reply primitive so managed agents can complete asks through structured control events instead of arbitrary terminal text.

## Decision

Add a live-control structured ask/reply path:

- `wardian ask <agent>` creates a backend-owned `request_id` and sends the prompt with an explicit instruction to respond with `wardian reply <request-id> --status done --stdin`.
- `wardian reply <request-id> --status done|blocked|failed --stdin` records the reply through the live control endpoint.
- Default `wardian ask` waits for the structured reply event and returns `request_id`, `reply.status`, `reply.body`, delivery evidence, watch events, and retained output.
- Explicit `wardian ask --until output:<token>` remains as the output-matching fallback for manual compatibility.
- Existing `wardian send`, `send --wait-until`, and `agent watch --until output:<token>` behavior remains watch/output based.

The first slice is live-only. Pending requests are stored in app memory and are not restored after app restart.

## Trust Boundary

When `wardian reply` runs from a Wardian-managed agent terminal, `WARDIAN_SESSION_ID` identifies the caller and the backend rejects replies from the wrong target session. Replies from ordinary terminals do not have reliable caller identity in this slice, so they are accepted for a known pending request to let a human unblock work. This is intentional and documented as unauthenticated local live-control behavior.

## Event Model

The backend emits watch events for inspectability:

- `request`: created when an ask request is registered.
- `reply`: emitted when a terminal reply is accepted.
- Existing `delivery` events continue to record input submission evidence.

Duplicate replies are rejected with `duplicate_reply`. Unknown request ids are rejected with `not_found`. A missing reply before the requested timeout returns `watch_timeout`, distinct from `blocked` or `failed` reply statuses.

## Compatibility

The previous Codex repaint echo guard is retained for explicit `--until output:<token>` waits. It is still needed because output matching remains supported and may be used with agents or workflows that cannot call `wardian reply`.

No provider runtime arguments, session resume behavior, sandbox policy, projected homes, or clear-session behavior are changed by this decision.

