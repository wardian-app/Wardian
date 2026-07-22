# PTY Lifecycle and Process Integrity

Wardian is built to handle multiple simultaneous, long-running agent sessions with strict resource and process isolation.

## 🌉 Cross-Platform PTY Layer
Wardian utilizes the `portable-pty` crate to provide a consistent PTY interface across different operating systems.

- **Windows**: Uses **ConPTY** (Windows Pseudo Console) through the `NativePtySystem`.
- **Linux/macOS**: Uses the standard Unix PTY system.

### The PTY Model:
- **Master**: The control end of the PTY, used for reading output and writing input.
- **Slave**: The application end, where the selected runtime shell hosts the provider command.

## 🛡️ Process Integrity (Windows Job Objects)
To prevent orphaned provider and console-host processes when Wardian crashes or is force-closed, the Windows implementation uses **Job Objects** via the `win32job` crate.

1. On startup, Wardian creates an app-lifetime `win32job::Job`.
2. The `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` flag is enabled.
3. Wardian assigns the backend process to that job before restoring or spawning interactive agents.
4. Provider shells, CLIs, ConPTY console hosts, and descendants inherit the job from process creation time.
5. When the Wardian process terminates, the job object is closed by the OS, which automatically kills all processes assigned to it.

Per-agent process-tree termination is still used for normal UI actions such as kill, pause, resume, and clear. Per-agent Job Objects are only a fallback if app-level supervision cannot be installed, because post-spawn assignment is inherently less reliable than inheriting the app-level job at creation time.

At startup, Wardian also sweeps stale persisted interactive sessions before restoring agents. This catches process trees from older builds or from environments where Windows refused app-level job assignment. The sweep uses Wardian session command-line markers and `WARDIAN_SESSION_ID` environment markers, and skips agents that are off or database-marked as headless.

## 🔁 Spawning Lifecycle
Spawning an agent follows a deterministic sequence in `manager::spawn_agent`:

1. **Open PTY**: Create a new master/slave pair.
2. **Resolve Runtime Shell**: Select the configured shell profile (`Auto`, discovered shell, or `Custom`).
3. **Build Provider Command**: Assemble the provider executable plus provider-specific flags from the selected `AgentConfig.provider_config`.
4. **Wrap for Host Shell**: Convert the provider command into a shell-hosted invocation that respects the selected shell family.
5. **Spawn**: The PTY slave spawns the shell-hosted command.
6. **Piping**:
   - A **Writer Thread** is spawned to handle input from the UI.
   - A **Reader Thread** is spawned to capture output, parsing it for JSON logs and status transitions.
7. **Registration**: The `ActiveAgent` handle is added to the `AppState`.

### Shell-hosted Launch Notes
- Workflow shell-command nodes and headless provider runs use the same shell resolver as interactive PTY sessions.
- On Windows, provider shims are host-aware: PowerShell hosts invoke `.cmd` and `.bat` shims directly through PowerShell, while POSIX-like hosts such as Git Bash or WSL may route Windows shims through `cmd.exe` for compatibility.
- On Linux and macOS, Wardian resolves shells from the standard shell list and executes the provider command through that shell's command-string mode.

## Input Readiness and Interaction Delivery

PTY input is a transport, not Wardian's communication source of truth. The interaction control plane owns structured messages, asks, replies, delivery attempts, and Inbox evidence. The PTY writer is only one possible way to deliver an interaction to a live provider runtime.

Each interactive provider runtime has a provider input generation. The generation increments whenever Wardian creates or reattaches a runtime boundary, including spawn, resume, clear, and provider reattach. Readiness observations are valid only for the generation that produced them.

```text
ProviderInputState {
  session_id,
  generation,
  state: unknown | booting | ready | busy | action_required | unavailable,
  ready_evidence: provider_event | prompt_detected | title_detected | manual_status,
  observed_at
}
```

Delivery follows these rules:

- Ready evidence for the current generation can drain queued interaction delivery.
- Booting, busy, action-required, unavailable, or missing input-sender states keep delivery queued with a precise reason.
- Readiness or status from an older generation cannot drain queued work for a newer runtime.
- Provider action-required status remains provider-owned. It usually represents a provider permission or authentication prompt, not a Wardian human-in-the-loop interaction.
- Codex readiness can use prompt detection as release evidence, but it must not depend on a fixed sleep before text injection.

This model prevents first-input races where Wardian writes into a provider before the provider prompt is actually ready. It also keeps Inbox and CLI behavior tied to durable interaction and provider events rather than terminal repaint artifacts.

## Testing Boundaries

PTY behavior cannot be validated by browser-only UI tests.

- Browser Playwright smoke tests are useful for layout, navigation, and non-native UI regressions.
- Native Tauri runtime tests are required for:
  - Tauri `invoke` behavior
  - PTY-backed terminal rendering
  - provider spawn and resume behavior
  - shell-hosted process launch behavior

When debugging or testing PTY issues, treat browser smoke results as insufficient evidence. Use the native runtime harness for any claim about terminal or provider behavior.

## 📐 Terminal Resizing

Terminal resizing is presentation-aware. Each visible terminal reports its
desired viewport without touching the PTY. Only the broker's active lease owner
may submit an epoch-bearing resize with a monotonically increasing geometry
sequence. The broker serializes native PTY resize, parser resize, canonical
geometry update, and the geometry stream event as one commit; stale, mirror, or
reordered requests are rejected nonfatally.

Desktop geometry is clamped to 20..500 columns and 8..200 rows. Remote geometry
uses 20..240 columns and 8..80 rows. Mirrors scale, pan, or letterbox the
canonical owner grid locally instead of applying smallest-client-wins geometry.
This keeps desktop rendering stable when a phone or narrow split is attached.

See [Terminal Presentation Broker](./terminal-presentation-broker.md) for the
generation, lease, snapshot, and ownership-transfer protocol.

## 🖥️ Frontend Terminal Runtime

Wardian's frontend terminal stack is built on `xterm.js` and is intentionally treated as a runtime layer, not just a view component.

### Renderer Strategy

- Wardian uses xterm's WebGL renderer for mounted terminal views when available. WebGL is preferred because xterm's `customGlyphs` support for block and box-drawing characters does not apply to the DOM renderer, and provider TUIs such as Claude Code rely on those glyphs for mascot/status rendering.
- If WebGL is unavailable or loses its context, Wardian falls back to xterm's built-in DOM renderer rather than failing terminal initialization.
- Renderer instances are not the source of runtime truth. Each presentation has
  an independent renderer, while the Rust broker parser, snapshots, and ordered
  event stream remain canonical if it must be recreated.
- Renderer retirement is lease-bound. Output/reset/refresh operations capture
  one renderer identity before awaiting; retirement releases its budget slot
  immediately but defers physical disposal until every in-flight operation
  finishes. Post-await work may mutate only the captured renderer generation.
- Agents keeps resident xterm renderers stable within the process budget while
  scrolling. Intersection controls WebGL promotion separately, so leaving the
  viewport does not cause destroy/recreate flicker. Above the budget, residency
  changes only when an approaching card needs capacity.
- Provider integrations must not depend on renderer-specific behavior.

### Capability Handling

Terminal capability negotiation is centralized in `src/features/terminal/terminalCapabilities.ts`.

That layer is responsible for responding to standard terminal queries such as:

- device status reports
- resize and pixel-size queries
- DECRQM mode checks
- OSC palette queries
- OSC 10/11 foreground and background color queries
- synchronized output toggles

Provider-specific terminal adapters should only exist when a provider genuinely requires non-standard behavior. Capability replies should otherwise be implemented once in the shared terminal layer.

### Broker Snapshot and Replay Model

Wardian preserves terminal state across presentation remounts and independent
desktop/remote renderers while the PTY runtime is live.

That means:

- switching tabs
- zooming or restoring groups
- remounting the terminal component

should not discard the active terminal buffer.

Terminal contents are runtime state and are never written into the workbench
document. A process restart can restore the tab but not a terminated PTY's
screen contents.

The session model is split into two layers:

- a Rust broker parser that continuously receives PTY output and owns canonical
  in-process screen, geometry, bounded snapshot, and replay state;
- independent mounted presentation terminals that consume one ordered stream
  and can be disposed and reconstructed from a snapshot/barrier.

When a presentation remains resident within the process-wide budget, Wardian
reuses its renderer across tab, layout, and viewport transitions. If it was
suspended or evicted, the presentation
applies a fresh bounded broker snapshot, discards events at or below the
snapshot barrier, and then replays consecutive later events. It must resync
again on a cursor gap or generation change.

### Redraw and Scrollback Normalization

Some TUIs repaint by moving the cursor home and rewriting the current viewport instead of using the alternate screen buffer. Wardian normalizes the cases that would otherwise diverge from user expectations:

- A clear-screen preamble made from many `EL + newline` writes followed by cursor-home is treated as a real clear-and-home operation. This prevents TUI redraws, such as Claude's mascot frame, from being copied into scrollback during maximize/restore.
- Synchronized home-redraw TUIs are marked as transient screen renderers. Before a row-shrinking resize, Wardian moves the local xterm cursor home so xterm does not promote the old visible TUI frame into scrollback before the provider redraws at the new size.
- After any resize, Wardian arms one duplicate-redraw suppression window. If the next synchronized home-redraw batch is mostly already present in the parser buffer, Wardian drops that repaint instead of letting xterm append a second copy of the same transcript to scrollback.
- Codex interactive sessions use its documented `--no-alt-screen` inline mode, and Wardian journals overlapping home-redraw frames into xterm scrollback. Codex still emits a sliding viewport, so Wardian reconstructs dropped frame lines before applying the next repaint.

### PTY Output Batching

The frontend drain path batches PTY output before writing into xterm instead of issuing one write per small chunk. This reduces render pressure during bursty output and improves scrolling behavior for TUI-heavy providers such as OpenCode.
