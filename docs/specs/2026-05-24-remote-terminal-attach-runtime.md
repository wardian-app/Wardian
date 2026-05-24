# Remote Terminal Attach Runtime

## Context

Wardian remote currently exposes terminal output as a bounded text snapshot. That is useful for logs, but it is the wrong abstraction for full-screen TUIs because cursor movement, alternate screen updates, colors, and transient redraws are meaningful terminal state rather than append-only text.

An attempted remote xterm replay path was removed because replaying raw bounded output into a second renderer is not reliable across providers or device widths. Wardian needs an OS-independent attach model with tmux-like semantics while keeping the Rust backend as the authority for agent lifecycle, PTY ownership, input routing, and remote access control.

The target is interactive remote terminal control. Opening a remote terminal should attach to the agent, take terminal ownership immediately, and route raw keyboard input through Wardian to the existing agent PTY.

## Goals

- Provide a Wardian-native interactive terminal attach model for remote clients.
- Avoid raw snapshot feeding as the rendering mechanism.
- Keep the default agent path lightweight when no remote terminal is attached.
- Preserve OS independence across Windows, macOS, and Linux.
- Keep Wardian, not an external multiplexer process, as the authoritative owner of PTY lifecycle, remote auth, audit, and input policy.
- Make remote attach semantics explicit: the attaching remote terminal becomes the active terminal owner immediately.

## Non-Goals

- Do not depend on external `tmux`, `psmux`, or platform-specific multiplexer binaries for the core feature.
- Do not attempt perfect historical reconstruction for already-running TUIs when no backend terminal parser was active before remote attach.
- Do not replace the desktop terminal renderer in the first slice.
- Do not expose unauthenticated terminal input or a second unaudited remote control path.

## Design

Wardian adds a lazy backend terminal attach runtime. The runtime is activated per agent only when a remote terminal attachment is opened.

Each active remote attachment has:

- an attachment id;
- the target agent session id;
- the authenticated remote session/device identity;
- current terminal geometry;
- an ownership state;
- a WebSocket transport for rendered terminal updates and terminal input.

When a remote client opens an agent terminal, Wardian creates an attachment and immediately marks it as the active terminal owner. The backend starts a rendered terminal screen runtime for that agent if one is not already warm, resizes the PTY to the remote terminal geometry, and begins streaming terminal screen state to the client. Remote key and binary input is routed through the same backend input sender used by existing desktop terminal commands.

The desktop terminal remains functional, but while a remote attachment owns the terminal it should be treated as an observer unless the desktop explicitly reclaims control. Reclaiming control is a later UI policy decision; the backend contract should support ownership transfer without relying on hidden renderer state.

## Lazy Runtime Policy

The runtime is demand-driven:

1. When no remote terminal is attached, existing PTY watch/transcript buffering remains the only backend output path.
2. When a remote terminal attaches, Wardian starts a backend terminal screen parser for that agent.
3. The parser consumes live PTY bytes while one or more remote terminal attachments exist.
4. When the last remote terminal detaches, the parser stays warm for a short grace period.
5. If no new remote attachment arrives before the grace period expires, Wardian disposes the parser and returns the agent to the lightweight baseline path.

The first remote frame is best effort for an already-running TUI. Remote attach triggers a PTY resize, which should cause most TUIs to repaint. From that repaint onward, Wardian streams from one authoritative backend terminal state instead of replaying a raw log into an independent renderer.

## Backend Components

Add a terminal attach domain under the Rust backend, separate from remote gateway routing and provider spawning.

Recommended components:

- `TerminalAttachState`: process-wide registry keyed by agent session id.
- `TerminalAttachRuntime`: per-agent lazy runtime containing parser state, attachments, current owner, geometry, and warm-dispose timer.
- `TerminalAttachment`: per-client metadata and outbound update channel.
- `TerminalScreenSnapshot`: serializable current rendered screen state.
- `TerminalScreenDelta`: serializable incremental screen update.
- `TerminalInputMessage`: remote-to-backend input envelope for UTF-8 data, binary bytes, resize, and detach.

The runtime must not own the PTY process. It attaches to the existing Wardian PTY lifecycle and uses existing input senders and resize commands. This preserves Wardian's current provider launch, pause, resume, kill, clear, job cleanup, and audit behavior.

## Remote Gateway

Add a terminal WebSocket path using the existing remote authentication and ticket model.

Proposed flow:

1. Remote client requests a WebSocket ticket for `terminal_attach`.
2. Client opens `/remote/api/agents/{session_id}/terminal-stream`.
3. Client sends the ticket and initial geometry.
4. Gateway validates the ticket, remote session, CSRF boundary where applicable, and target agent access.
5. Gateway creates the backend attachment.
6. Gateway forwards outbound terminal snapshots/deltas to the client.
7. Gateway accepts input, binary input, resize, and detach messages from the current owner only.

The existing text terminal snapshot endpoint may remain as a log fallback, but the remote terminal UI should not use it for interactive rendering.

## Ownership And Resize Semantics

Opening a remote terminal immediately takes ownership.

The active owner controls:

- PTY resize requests;
- raw keyboard input;
- binary input;
- paste/input batching policy.

When ownership changes, Wardian broadcasts the new owner and geometry to all attachments. Non-owner attachments may continue receiving rendered screen updates but cannot write input or resize unless promoted to owner.

If the owner disconnects, Wardian detaches it and either promotes the most recent remaining attachment or clears ownership. If no remote attachments remain, the backend returns to the lazy warm-dispose path.

## Frontend Remote UI

The mobile remote terminal should use a real terminal component connected to the terminal WebSocket. It should not poll terminal snapshots for rendering.

On open, it sends measured terminal columns and rows. On resize/orientation changes, it sends resize messages while it remains owner. Input comes from the terminal component's data and binary callbacks and is forwarded to the WebSocket.

The UI should make ownership visible only when useful. The default behavior is that the open remote terminal is active and interactive.

## Performance

The feature must avoid parsing every running agent by default.

Performance guardrails:

- no backend screen parser unless a remote terminal attach is active or warm;
- bounded scrollback and bounded outbound message buffers;
- backpressure behavior for slow remote clients;
- coalesced screen updates at a modest frame rate;
- warm parser disposal after a short grace period;
- no unbounded raw output replay on attach.

The parser may be seeded from recent bounded PTY output only as best effort, but correctness must come from live bytes after attach and PTY resize, not from raw snapshot replay.

## Testing

Use the lowest layer that proves each behavior:

- Rust unit tests for attachment lifecycle, owner selection, resize policy, warm disposal, and input authorization.
- Remote gateway tests for ticket validation, authenticated attach, rejected stale owner input, and detach cleanup.
- Browser tests for remote UI connection state, resize message emission, and keyboard forwarding.
- Native runtime E2E for real PTY behavior: attach, remote owns terminal, resize occurs, typed input reaches the provider PTY, and rendered screen updates continue after a TUI repaint.

Browser E2E alone is not enough for claims about PTY fidelity.

## Open Decisions

- Parser choice: use the Rust `vt100` crate for the first implementation because it provides a backend terminal parser, current screen state, formatted full-state output, and formatted diffs without requiring a platform-specific multiplexer process.
- Protocol shape: the first implementation sends a full formatted terminal state on attach and after missed updates, then sends formatted ANSI diffs for live updates.
- Ownership reclaim behavior from the desktop UI.
- Warm parser grace duration: keep the parser warm for 60 seconds after the last remote attachment detaches.
- Whether remote attachments should show scrollback or only the active viewport in the first implementation.
