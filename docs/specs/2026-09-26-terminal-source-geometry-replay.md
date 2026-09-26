# Terminal Source Geometry Replay

## Context

Broker snapshots contain formatted VT state at the PTY's canonical columns and
rows. Desktop panes can be narrower than that grid. Replaying formatted bytes
into a pane-sized xterm wraps the source frame, shifts right-edge content, and
can alter color, cursor, and mode state. The old desktop fallback used the
snapshot's plain visible grid when dimensions differed, losing formatting.

## Decision

- Size each desktop xterm to `snapshot.geometry` before replaying formatted
  state. Keep the pane viewport separate, then scale, letterbox, or pan the
  source grid with the existing mirror-fit policy. A passive mirror never
  resizes the PTY.
- A geometry-commit or activation acknowledgement snapshot advances the event
  barrier but does not prove that the provider has repainted. Keep the last
  accurate frame and gate automatic input until ordered output after the
  geometry change permits one fresh session snapshot shared by presentations.
- If no output arrives, retain the old frame and show a pending status. An
  acknowledged owner may explicitly enable keyboard input to prompt recovery,
  with an unseen-prompt warning. Mouse-coordinate encodings and binary input
  stay gated; ownership transfer revokes the allowance. Wardian sends no
  synthetic redraw key and runs no retry timer.
- During the first owner geometry transition, ordinary typed text can pass
  while repaint is pending once the reported viewport matches the broker's
  committed geometry. Escape-sequence and binary input remain gated. After
  a post-geometry snapshot resolves that first transition as ready or degraded,
  later silent resizes require the explicit keyboard recovery action above.
- Retain local Codex scrollback when a geometry boundary snapshot contains no
  broker history; a later formatted frame updates the visible screen without
  discarding that local history.
- The broker's 2 MiB limit trims history before omitting an oversized formatted
  state. Desktop replay identifies missing and invalid formatted payloads as
  degraded. It shows a plain projection only when no accurate previous frame
  exists and identifies the loss of formatting in the UI.

## Alternatives considered

- Blind retry or timer-driven resize can cause a resize loop and cannot make an
  uncooperative provider repaint.
- Treating a geometry acknowledgement as repaint evidence can replace an
  accurate frame with stale state.
- Relying only on later output without a fresh snapshot leaves passive mirrors
  incomplete when the provider repaints only part of the screen.
- Resizing the PTY to every pane, or replaying formatted bytes into a pane-sized
  grid, makes passive mirrors change or corrupt the shared terminal.

## Verification and limits

Component and real xterm-parser tests cover source-width right-edge content,
SGR color, mirror geometry, owner pending state, explicit keyboard recovery,
and degraded payloads. The isolated mock-PTY native path checks owner and
mirror source geometry and formatted replay. Provider-specific repaint
behavior and any terminal protocol that lacks a formatted broker state remain
bounded by the explicit degraded status.
