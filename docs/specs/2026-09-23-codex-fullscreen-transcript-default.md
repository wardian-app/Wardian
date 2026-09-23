# Codex fullscreen transcript default

## Status

Implemented for Codex 0.156.0 and later; native acceptance is tracked with the change.

## Problem

Wardian forced Codex's `--no-alt-screen` mode so terminal scrollback could hold
the conversation. Codex's inline composer follows the content rather than the
bottom of a taller terminal. Resizing or maximizing a Wardian card therefore
leaves blank rows below the composer. The terminal geometry itself is correct;
resizing it again cannot move the inline composer to the bottom. Codex 0.155.0
also leaves animated decorative cells in some normal-buffer captures.

## Decision

Wardian no longer forces `--no-alt-screen` for a managed Codex TUI. During the
agent-local Codex home projection, it sets `[tui].fullscreen_transcript = true`
when that choice is absent and `alternate_screen` is not `"never"`. It preserves
explicit agent or inherited choices, including Scrollback and disabled
alternate screen. The projected config remains agent-owned and inspectable;
Wardian does not change the user's global Codex config.

Codex owns its fullscreen transcript and composer. Wardian owns the PTY size,
canonical output, and terminal presentation. A fullscreen Codex terminal uses
the alternate buffer, so Wardian's normal-buffer xterm scrollback is no longer
the transcript navigation surface. The existing broker output and snapshot
contracts still apply. Broker snapshots record the active screen mode, enter
the alternate buffer before replaying a formatted frame, and preserve that
mode when a receiver must use the plain-text fallback. Wardian does not
synthesize scrollback or move cursor cells to imitate a different Codex mode.

## Compatibility and migration

Codex 0.156.0 introduced the fullscreen transcript setting. Older Codex
versions remain usable but keep their native inline behavior. Users can choose
`/tui Scrollback` in Codex, or set `alternate_screen = "never"`, and Wardian
preserves that choice on subsequent home syncs. The new default takes effect
on the next Codex TUI launch; an already-running process keeps its current
screen mode. Other providers' launch arguments and terminal modes are unchanged.

Native acceptance must verify the alternate buffer and bottom composer across
resize, maximize, minimize/restore, and pause/resume. It must distinguish an
unsubmitted draft from a provider turn. The earlier terminal scrollback
ownership contract still governs normal-buffer providers and an explicit
Codex Scrollback choice.
