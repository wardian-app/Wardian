# OpenCode terminal selection consistency

- **Status:** Implemented
- **Date:** 2026-07-03
- **Area:** Frontend terminal input, OpenCode provider behavior
- **Superseded in part:** The 2026-07-07 mouse-tracking suppression decision is
  superseded by `2026-07-09-opencode-terminal-protocol-ownership.md`.

## Context

PR 589 added an OpenCode-only exception that disabled xterm stdin when an
OpenCode terminal card was not selected. The intent was to keep passive
mouse/wheel protocol reports from reaching OpenCode while users moved around the
grid.

PR 618 later added a provider-scoped input guard that filters OpenCode passive
mouse-motion reports before Wardian forwards frontend `onData` or `onBinary`
input to the PTY. That guard fixes the observed composer garbage characters
without disabling terminal stdin.

## Decision

Wardian keeps the existing default terminal interaction model for OpenCode:
terminal stdin remains enabled regardless of grid selection. This preserves
direct terminal scrolling and keeps OpenCode behavior consistent with the other
providers.

Selection remains a grid concern for card chrome, actions, and focus routing. It
does not change xterm stdin. OpenCode-specific safety remains in
`filterProviderTerminalInput`, which strips passive mouse-motion reports while
preserving typed input and non-motion mouse packets such as wheel reports.

Update 2026-07-07 (superseded 2026-07-09): OpenCode's TUI can emit xterm mouse-tracking enables
(`ESC[?1000h`, `ESC[?1002h`, `ESC[?1003h`, `ESC[?1006h`, and `ESC[?1016h`).
When those toggles reach xterm.js, plain drag gestures are converted into mouse
protocol input instead of normal text selection. Wardian now strips those
OpenCode-only mouse-tracking toggles in `normalizeOpenCodeOutput` before xterm
sees them. This keeps normal text selection available without changing grid
selection or disabling terminal stdin.

Wardian still preserves the passive mouse-motion input guard as defense in
depth for already-active sessions and older scrollback, but OpenCode-owned mouse
tracking is no longer a supported interaction path inside Wardian terminals.

## Verification

Regression coverage lives in:

- `src/features/terminal/AgentTerminal.test.tsx`: unselected OpenCode terminals
  keep `disableStdin` false, capability replies are still sent, and passive
  mouse-motion bytes are not forwarded.
- `src/views/GridView.test.tsx`: grid selection state is no longer passed into
  `AgentTerminal` input props.
- `src/features/terminal/terminalCapabilities.test.ts`: OpenCode motion reports
  are filtered, OpenCode mouse-tracking toggles are stripped before rendering,
  and non-OpenCode inputs are preserved.
