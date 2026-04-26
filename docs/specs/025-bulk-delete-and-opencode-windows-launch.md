# Spec 025: Bulk Delete Confirmation and OpenCode Windows Launch

- **Status:** Implemented
- **Date:** 2026-04-25
- **Decider:** User

## Context and Problem Statement

Two small but high-friction behaviors needed tightening:

1. Multi-selected agents in the watchlist could route bulk delete through repeated single-agent confirmations instead of one confirmation for the whole target set.
2. OpenCode launches on Windows could fail when `PATH` resolved to a shell shim rather than a native executable, especially under direct PTY/process spawning where Windows does not apply shell command dispatch automatically.

These both sit on boundary behavior that users already have strong expectations about: Explorer-style bulk actions in the roster, and predictable provider startup on Windows.

## Proposed Decision

### Bulk delete confirmation

- Multi-selection context menus continue to resolve to the full current selection when the right-click target is inside that selection.
- Delete now has a dedicated bulk callback path instead of looping through the single-agent delete callback.
- The confirmation dialog is shown exactly once for the resolved selection.
- The confirmation copy is selection-sized:
  - `Delete this agent?`
  - `Delete N selected agents?`
- After confirmation, deletions are executed sequentially and watchlist selection/off-state cleanup is applied once for the full deleted set.

### OpenCode Windows executable resolution

- On Windows, OpenCode executable lookup now prefers native launch targets over shell shims:
  - direct `opencode.exe`
  - packaged OpenCode binary discovered behind a shim layout
  - shim fallback only if no native executable is found anywhere in `PATH`
- Interactive OpenCode launches still wrap non-`.exe` shim targets through `cmd /d /c ...` because Wardian launches providers directly through Windows PTY/process APIs rather than via the user's selected shell.
- This preserves shell independence at the Wardian layer while avoiding `CreateProcessW` failures for `.cmd`/script shims.

## Consequences

- **Positive**: Bulk delete now matches file-explorer expectations and reduces destructive-action noise.
- **Positive**: OpenCode startup on Windows prefers native binaries when available and remains compatible with shim-only installs.
- **Positive**: The Windows launch rule is explicit and testable instead of depending on ambient shell behavior.
- **Negative**: OpenCode Windows launch resolution now has slightly more branching logic in the provider adapter.
- **Negative**: PTY launch behavior still needs a Windows shell wrapper when the resolved target is only a shim, even though the user's interactive shell choice is preserved elsewhere.
