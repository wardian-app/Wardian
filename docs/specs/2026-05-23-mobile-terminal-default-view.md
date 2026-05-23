# Mobile Terminal Default View

Filename: `2026-05-23-mobile-terminal-default-view.md`

- **Status:** Implemented
- **Date:** 2026-05-23
- **Decider:** User

## Context and Problem Statement

Wardian's remote mobile PWA currently opens an agent into a chat-first conversation view. Chat mode is useful, but it is still incomplete and can hide the provider state that is most useful on a phone during active debugging. The desktop terminal remains the authoritative view of an agent session.

The remote mobile agent detail view should show terminal output by default while keeping chat available through a quick mode switch. The terminal surface must not call the existing desktop `read_agent_pty` command because that command drains the frontend PTY buffer and can race the desktop terminal renderer.

## Decision

Add a remote terminal snapshot API:

`GET /remote/api/agents/{session_id}/terminal?since=<cursor>&tail_bytes=<bytes>`

The endpoint reads the agent's `AgentWatchState` and returns a non-draining, sanitized terminal snapshot:

- `cursor`: watch cursor for incremental refresh.
- `text`: sanitized terminal text.
- `truncated`: whether older bytes were omitted.
- `omitted_bytes`: omitted byte count.

The endpoint is authenticated and audited like the current remote chat read endpoint. It uses a safe bounded `tail_bytes` value so a phone cannot request unbounded terminal history.

On the frontend, replace the chat-only remote agent detail component with a mobile detail view that has a compact `Terminal | Chat` segmented control. `Terminal` is selected by default. The terminal pane renders a scrollable preformatted transcript, refreshes when status stream updates indicate active-agent changes, and keeps the existing prompt composer for sending input through the existing remote action endpoint. `Chat` reuses the current chat transcript rendering and composer path.

## Consequences

- **Positive**: The phone defaults to the most faithful current agent surface.
- **Positive**: The desktop terminal renderer is not starved by remote reads.
- **Positive**: Chat remains one tap away for normalized transcript reading.
- **Negative**: This is a readable terminal transcript, not a full interactive xterm renderer.
- **Negative**: Incremental refresh still depends on status stream changes and manual refresh rather than a dedicated terminal WebSocket.
