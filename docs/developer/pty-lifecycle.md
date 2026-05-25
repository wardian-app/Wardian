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
- On Windows, `.cmd` and `.bat` provider shims may be re-routed through `cmd.exe` when the selected host shell is PowerShell, Git Bash, or WSL.
- On Linux and macOS, Wardian resolves shells from the standard shell list and executes the provider command through that shell's command-string mode.

## Input Readiness and Interaction Delivery

PTY input is a transport, not Wardian's communication source of truth. The interaction control plane owns structured messages, asks, replies, delivery attempts, and Queue evidence. The PTY writer is only one possible way to deliver an interaction to a live provider runtime.

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

This model prevents first-input races where Wardian writes into a provider before the provider prompt is actually ready. It also keeps Queue and CLI behavior tied to durable interaction and provider events rather than terminal repaint artifacts.

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
Terminal resizing is handled asynchronously in `manager::resize_pty`. When the UI grid layout changes, it invokes a Tauri command that updates the PTY dimensions (`rows` and `cols`) via the `pty_master` handle, ensuring the agent's TUI renders correctly.

## 🖥️ Frontend Terminal Runtime

Wardian's frontend terminal stack is built on `xterm.js` and is intentionally treated as a runtime layer, not just a view component.

### Renderer Strategy

- Wardian uses xterm's WebGL renderer for mounted terminal views when available. WebGL is preferred because xterm's `customGlyphs` support for block and box-drawing characters does not apply to the DOM renderer, and provider TUIs such as Claude Code rely on those glyphs for mascot/status rendering.
- If WebGL is unavailable or loses its context, Wardian falls back to xterm's built-in DOM renderer rather than failing terminal initialization.
- Renderer instances are not the durable source of truth. Wardian reuses a live renderer across ordinary pane moves, but the parser state remains canonical if a renderer must be recreated.
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

### In-App Replay Model

Wardian preserves terminal state across UI remounts inside the running app process.

That means:

- switching views
- maximizing or restoring panes
- remounting the terminal component

should not discard the active terminal buffer.

This is intentionally scoped to the current app process only. Full restart persistence is still out of scope.

The session model is split into two layers:

- a detached parser terminal that continuously receives PTY output and owns the canonical in-app screen state
- a mounted view terminal that can be disposed and recreated without losing that state

When a terminal view remounts and the existing renderer is still valid, Wardian reattaches that renderer. If a renderer must be recreated, Wardian restores it from the parser terminal's serialized state instead of replaying raw PTY chunks into a fresh xterm view.

### Redraw and Scrollback Normalization

Some TUIs repaint by moving the cursor home and rewriting the current viewport instead of using the alternate screen buffer. Wardian normalizes the cases that would otherwise diverge from user expectations:

- A clear-screen preamble made from many `EL + newline` writes followed by cursor-home is treated as a real clear-and-home operation. This prevents TUI redraws, such as Claude's mascot frame, from being copied into scrollback during maximize/restore.
- Synchronized home-redraw TUIs are marked as transient screen renderers. Before a row-shrinking resize, Wardian moves the local xterm cursor home so xterm does not promote the old visible TUI frame into scrollback before the provider redraws at the new size.
- After any resize, Wardian arms one duplicate-redraw suppression window. If the next synchronized home-redraw batch is mostly already present in the parser buffer, Wardian drops that repaint instead of letting xterm append a second copy of the same transcript to scrollback.
- Codex interactive sessions use its documented `--no-alt-screen` inline mode, and Wardian journals overlapping home-redraw frames into xterm scrollback. Codex still emits a sliding viewport, so Wardian reconstructs dropped frame lines before applying the next repaint.

### PTY Output Batching

The frontend drain path batches PTY output before writing into xterm instead of issuing one write per small chunk. This reduces render pressure during bursty output and improves scrolling behavior for TUI-heavy providers such as OpenCode.
