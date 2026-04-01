# Spec 011: Codex Terminal History Preservation

## Context

Wardian mounts agent terminals inside a dynamic grid. Panels can remount during layout changes, focus changes, or view transitions.

That remount behavior is acceptable for line-oriented CLIs, but Codex uses a full-screen TUI with cursor movement, alternate-screen usage, and mode transitions. Wardian previously cached raw PTY chunks and replayed them into a fresh xterm instance on remount. That does not faithfully reconstruct Codex's rendered screen or scrollback.

The practical failure mode was: Codex history looked correct when launched directly in a native terminal, but appeared incomplete or incorrect after Wardian remounted the terminal pane.

Trace-based debugging later showed two renderer mismatches in Wardian's live terminal path. First, Codex inline mode emits an immediate cursor-position query (`CSI 6 n`) and uses the answer to place its viewport and visible history rows. Wardian first delayed PTY reads behind a fit/resize readiness check, then continued to drain PTY output on a timed polling loop with awaited `term.write(...)` callbacks. Under Codex's repaint-heavy inline mode that introduced seconds of extra latency, so CPR replies landed too late and scrollback accumulated stale frame artifacts instead of the latest logical transcript.

The VS Code source showed a more important architectural difference: its terminal host keeps a long-lived xterm instance and attaches/detaches that live instance from the DOM, rather than disposing and recreating xterm on view remounts. In Wardian, React remounts were tearing down the active parser/buffer state and then trying to reconstruct it after the fact. For Codex's synchronized repaint stream, that was the wrong lifecycle.

## Decision

Wardian will keep one live xterm instance per agent session, move PTY output delivery from timer polling to backend readiness events, and only attach/detach the xterm host element when the React pane remounts.

- Replace timed PTY polling with a lightweight backend `agent-pty-output-ready` event. The PTY reader thread remains the source of truth by appending to `output_buffer`, and the frontend drains that buffer immediately when notified.
- Keep the xterm parser, buffer, scrollback, and live terminal modes in memory for the lifetime of the agent session instead of disposing them on pane remount.
- On pane remount, move the existing xterm host element back into the new React container and refit it, following the same broad model used by VS Code's `TerminalInstance` and `XtermTerminal`.
- Preserve xterm binary input separately from text input so mouse-report bytes from full-screen TUIs can reach the PTY unchanged.
- Start PTY reads immediately after the xterm instance opens instead of waiting for fit/resize readiness. Terminal IO must not be gated on layout measurement because Codex issues early terminal-control probes during inline-mode startup.
- Send xterm text input through the direct `send_input_to_agent` command path rather than an app-wide Tauri event bridge. This keeps cursor-position replies and other terminal-generated control input on the lowest-latency path.
- Launch Codex with `--no-alt-screen` inside Wardian so xterm keeps scrollback in the primary buffer instead of moving Codex into an alternate buffer with effectively no terminal scrollback.
- For Codex terminals, enable xterm's `scrollOnEraseInDisplay` behavior so clear-screen redraws in the primary buffer push previous content into scrollback instead of wiping the viewport.
- Keep xterm `reflowCursorLine` disabled, which matches the xterm default for PTY-backed shells and avoids xterm mutating shell-managed lines during resize.
- Strip Codex `CSI 3 J` scrollback-erase sequences before writing PTY output into xterm. xterm.js honors that sequence by deleting saved lines, while the Windows console VT documentation only defines `CSI J` values `0`, `1`, and `2`, which makes `3J` a plausible source of host-specific history loss.

## Consequences

- **Positive**: Codex terminals retain the same rendered state across Wardian remounts, matching native-terminal behavior much more closely.
- **Positive**: Codex wheel and mouse reports can reach the PTY in the same raw-byte form xterm emits, which is necessary for native-like scrolling inside alternate-screen TUIs.
- **Positive**: Codex now receives cursor-position replies during startup on the same timeline as a native terminal, which keeps inline-mode viewport placement and history tracking coherent.
- **Positive**: PTY repaint bursts no longer wait for a 50 ms poll interval plus chained callback latency before reaching xterm, which reduces lag and stale-frame accumulation for Codex.
- **Positive**: React remounts no longer destroy xterm parser/buffer state, so Codex's synchronized repaint stream keeps its real terminal context instead of being replayed into a fresh emulator.
- **Positive**: Codex history now accumulates in Wardian's normal terminal scrollback instead of being trapped in an alternate-screen viewport.
- **Positive**: Codex redraws that clear the screen in primary-buffer mode can still preserve prior content in xterm scrollback.
- **Positive**: Codex-specific scrollback erase sequences no longer wipe xterm history in Wardian when native Windows terminals would not.
- **Positive**: The fix stays in the frontend terminal layer and does not add provider-specific PTY behavior in Rust.
- **Positive**: Other providers benefit from more faithful remount restoration without changing their launch/runtime paths.
- **Negative**: Terminal state preservation now depends on xterm serialize-addon compatibility with the pinned xterm version.
- **Negative**: Live xterm instances now stay resident per open session until the session is explicitly cleaned up, which increases frontend memory retention relative to disposing on every remount.
- **Negative**: PTY output readiness is now another backend event contract the frontend must subscribe to correctly.
- **Negative**: Wardian still maintains separate text and binary terminal-input paths, which slightly increases frontend/backend terminal integration complexity.
- **Negative**: Codex no longer uses its full alternate-screen presentation inside Wardian, so the UI behavior is intentionally biased toward scrollback fidelity over strict TUI presentation parity.
- **Negative**: Wardian now deliberately diverges from strict xterm handling for one Codex-specific control sequence in order to match user-visible Windows terminal behavior more closely.
- **Negative**: Snapshot accuracy is bounded by xterm serialization semantics, not by a byte-for-byte PTY transcript.
