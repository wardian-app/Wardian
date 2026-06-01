# Codex Terminal Scrollback Backfill

## Context

Codex terminal sessions can retain a large PTY output buffer before the React
terminal view mounts. The previous frontend drain path waited for the full
destructive backend read to complete before writing any output to xterm. In
long conversations, that made the initial terminal paint wait on historical
scrollback parsing even when the most recent frame was enough to make the
session usable.

This issue is specific to agent terminal rendering. Chat transcript history is
handled by a separate data path and should not be used as the fix surface for
terminal startup latency.

## Decision

Add an optional `read_agent_pty` peek mode that can return a bounded recent PTY
tail without draining the retained backend buffer. Codex agent terminals use it
only on their first drain after mount:

- read up to 128 KiB of the recent PTY tail with `peek: true`;
- render that tail immediately without recording it as durable queue output;
- drain the full PTY buffer in the background with the existing destructive
  read;
- reset the parser and visible renderer before replaying the full buffer, so
  preserved scrollback remains intact and queue summaries see the canonical
  output once.

Non-Codex providers keep the previous destructive-read behavior. That avoids
changing OpenCode capability probe timing and preserves existing terminal
semantics outside the Codex scrollback path.

## Consequences

Codex terminal startup can paint a recent frame before the full retained
scrollback has been parsed, reducing perceived startup lag in long sessions.
The full replay can still take time for very large histories, but it happens
after the terminal has shown useful current state.

The backend command remains backward compatible for existing callers because
omitting options still drains the buffer exactly as before.
