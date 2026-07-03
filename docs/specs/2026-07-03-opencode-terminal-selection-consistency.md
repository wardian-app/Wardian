# OpenCode terminal selection consistency

- **Status:** Implemented
- **Date:** 2026-07-03
- **Area:** Frontend terminal input, OpenCode provider behavior

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

## Verification

Regression coverage lives in:

- `src/features/terminal/AgentTerminal.test.tsx`: unselected OpenCode terminals
  keep `disableStdin` false, capability replies are still sent, and passive
  mouse-motion bytes are not forwarded.
- `src/views/GridView.test.tsx`: grid selection state is no longer passed into
  `AgentTerminal` input props.
- `src/features/terminal/terminalCapabilities.test.ts`: OpenCode motion reports
  are filtered while wheel packets and non-OpenCode inputs are preserved.
