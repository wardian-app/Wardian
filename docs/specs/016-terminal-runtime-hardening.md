# Spec 016: Terminal Runtime Hardening

* **Status:** Proposed
* **Date:** 2026-04-09
* **Decider:** Architect

## Context and Problem Statement

Wardian's terminal layer is now carrying too much provider-specific behavior and still has visible quality issues across providers:

- scrolling performance degrades under heavy output
- remounts rely on ad hoc in-memory preservation instead of a clear replay model
- terminal capability handling is partly provider-specific, especially for OpenCode
- theme and color behavior remain inconsistent because Wardian does not yet present a sufficiently complete terminal capability surface

Wardian already uses the correct broad primitives:

- `portable-pty` on the Rust side as the cross-platform PTY layer
- `xterm.js` on the frontend as the terminal emulator

So the problem is not that Wardian chose the wrong base stack. The problem is that the runtime contract between PTY, transport, emulator, and renderer is still too thin and too ad hoc.

The current implementation also risks long-term technical debt by continuing to patch provider-specific terminal quirks in `AgentTerminal.tsx`.

## Decision

Wardian will harden the terminal runtime in a first pass focused on in-app correctness and rendering quality, while explicitly avoiding a full app-restart persistence layer.

This pass will:

- improve rendering correctness during resize and remount
- preserve terminal state across UI remounts within a running app session
- centralize terminal capability emulation into a provider-neutral layer
- reduce provider-specific terminal branches
- improve PTY output buffering so large output bursts do not degrade scrolling as severely

## Architecture

### 1. Keep the Current Base Stack

Wardian will keep:

- `portable-pty` for backend PTY management
- `xterm.js` for frontend terminal emulation

This is intentionally closer to VS Code's architectural shape without trying to reproduce Electron-specific infrastructure directly.

Wardian does **not** need to replace `portable-pty` with another PTY abstraction. The equivalent of VS Code's `node-pty` is already present in the current Rust/Tauri architecture.

### 2. Separate Parsed Terminal State from Mounted Renderer State

Wardian will not treat the mounted xterm renderer as the source of truth.

The session model will be split into:

- a detached parser terminal that continuously receives PTY output
- a mounted view terminal used only for visible rendering and input

When a view remounts, Wardian should reuse the live renderer if it is still valid. If the renderer must be recreated, Wardian restores it from serialized parser state instead of replaying raw PTY chunks directly into a fresh xterm view.

This keeps renderer lifecycle bugs from becoming terminal-state bugs and is closer to the VS Code split between terminal state ownership and terminal rendering.

Mounted terminal views should prefer xterm's WebGL renderer when available. WebGL enables xterm's custom glyph path for block and box-drawing characters, which is required for provider TUIs that render pixel-art/status UI with block glyphs. If WebGL initialization or context retention fails, the terminal must fall back to the built-in DOM renderer without breaking the session.

### 3. Introduce a Terminal Capability Broker

Wardian will replace most provider-specific terminal query/reply handling with a capability broker that owns terminal emulation for standard terminal queries and responses.

This broker will be responsible for:

- device status reports / cursor position replies
- terminal pixel-size and resize replies
- DECRQM handling
- palette and standard color queries
- focus in/out handling
- synchronized output toggles
- other standard capability negotiations that providers expect from a modern terminal

The broker should support at least:

- OSC palette handling already needed by OpenCode
- foreground/background color queries such as OSC 10/11 if present in real traces
- normalization for terminal redraw patterns that are standard escape-sequence compositions but produce poor scrollback in embedded xterm views

Provider-specific logic should only remain where a provider genuinely departs from standard terminal behavior.

### 4. Add Explicit In-Memory Replay Ownership

Wardian will keep terminal replay only for the lifetime of the running app process in this pass.

That means:

- terminal state survives pane remounts, layout changes, and view switches
- terminal state does **not** yet survive full app restart

Replay ownership should become explicit and parser-owned:

- the detached parser terminal is the canonical in-app state owner
- mounted terminal views are reattached when possible and restored from serialized parser state only when recreation is required
- remounting should not depend on raw PTY replay
- remounting should not rely on replaying raw PTY chunks into a brand-new renderer

This is meant to make remount behavior deterministic and easier to debug.

### 5. Improve PTY Output Transport and Buffering

Wardian's PTY transport should be tightened so the frontend is not overly dependent on repeated tiny poll/drain cycles.

The first pass should improve:

- batching of PTY output chunks
- replay-friendly buffering
- resistance to output bursts that currently degrade scrolling and repaint behavior

This should remain compatible with the current Tauri command/event model, but the data path should become more deliberate and less fragile.

### 6. Normalize Home-Redraw TUI Scrollback

Several provider TUIs redraw by moving the cursor home and repainting the screen. In a compact embedded terminal, two patterns need explicit handling:

- Some TUIs clear by writing many `EL + newline` sequences and then homing the cursor. Wardian should normalize that to a clear-and-home operation so resize redraws do not become duplicated scrollback.
- Some synchronized-output TUIs repaint from cursor-home while leaving the cursor near the bottom of the screen. Before row-shrinking resizes, Wardian should locally home the parser and renderer cursors so xterm does not promote the old transient frame into scrollback before the provider redraws.
- After a resize, a synchronized home-redraw that is mostly already present in the parser buffer should be treated as a duplicate repaint and dropped. This prevents long transcript redraws from being appended as new history when the provider is only repainting for the new geometry.
- Codex's inline TUI can emit a sliding home-redraw viewport. Wardian should run Codex in its documented `--no-alt-screen` mode and reconstruct dropped overlapping frame lines into xterm scrollback so users can scroll through prior output.

The Codex frame journal is intentionally provider-scoped because applying the same reconstruction to every home-redraw TUI corrupts Claude's mascot/status rendering.

## Scope

### Included

- detached parser-terminal plus mounted view-terminal lifecycle
- provider-neutral terminal capability broker
- in-memory replay across UI remounts within a running app
- PTY buffering improvements for smoother scrolling
- targeted native tests for PTY/runtime behavior

### Excluded

- full restart persistence like VS Code's headless replay across app relaunch
- replacing `portable-pty`
- a full dedicated external PTY host process
- provider-specific terminal customization beyond what is required to bridge genuinely non-standard behavior

## Testing Strategy

This work cannot be validated by browser-only Playwright.

The required evidence for terminal claims is:

- frontend unit coverage for replay/capability handling
- backend Rust tests for PTY/runtime buffer logic where applicable
- native Tauri runtime E2E for real PTY behavior
- real-provider native validation when a provider-specific terminal behavior is involved

Browser smoke tests remain useful for layout regressions, but they are not sufficient evidence for terminal correctness.

## Consequences

- **Positive**: scrolling and rendering quality should improve across all providers, not just OpenCode
- **Positive**: terminal handling becomes less provider-specific and easier to maintain
- **Positive**: remount behavior becomes deterministic and resilient
- **Positive**: OpenCode theme/capability debugging can move onto a cleaner terminal foundation
- **Negative**: the terminal stack becomes more structured and therefore more code moves through shared abstractions
- **Negative**: parser/view divergence must be prevented by keeping resize and capability handling synchronized
- **Negative**: full app-restart persistence remains intentionally out of scope for this pass
