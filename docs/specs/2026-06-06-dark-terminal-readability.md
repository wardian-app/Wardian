# Dark Terminal Readability

Filename: `2026-06-06-dark-terminal-readability.md`

- **Status:** Implemented
- **Date:** 2026-06-06
- **Decider:** Wardian maintainers

## Context and Problem Statement

Wardian's Grid remains terminal-first for direct provider control, but long-running
agent sessions need to be easier to scan in dark mode. Provider CLIs already emit
ANSI colors for prompts, commands, status text, warnings, errors, and summaries.
The previous embedded xterm theme used a near-black surface and left most ANSI
colors at xterm defaults, which made those semantic cues feel cramped and harder
to distinguish.

The solution must preserve raw PTY fidelity. Wardian should not rewrite provider
output into synthetic blocks in Terminal mode. Normalized Chat mode remains the
structured scanning surface, while Terminal mode should render raw bytes with
better typography and theme-aware ANSI colors.

## Proposed Decision

Use a shared frontend terminal theme module for both agent terminals and the
bottom user terminal:

- Define terminal-specific CSS variables in `src/styles/App.css` for background,
  foreground, cursor, selection, and the full normal/bright ANSI palette.
- Resolve those variables into xterm's `ITheme` through
  `src/features/terminal/terminalTheme.ts`, with dark/light fallback palettes for
  startup and test environments where the root `data-theme` attribute is not yet
  synchronized.
- Set xterm `lineHeight` to `1.25` for both terminal surfaces.
- Keep Wardian's existing VS Code-style platform font stacks so the embedded
  terminal remains familiar by default.
- Keep raw PTY output and provider ANSI escape sequences unchanged.

The dark terminal surface is intentionally less black and less purely ecological
than the surrounding app shell. It keeps a subtle Wardian tint but prioritizes
reading and scan speed over brand intensity.

## Consequences

- **Positive**: Prompts, processing output, success, warnings, errors, and muted
  summaries have explicit contrast-tested colors in dark mode.
- **Positive**: Agent terminals and the user terminal share the same readable
  xterm palette and row metrics.
- **Positive**: Provider raw output remains available for debugging because the
  renderer theme changes do not transform PTY bytes.
- **Negative**: More terminal-specific tokens must stay aligned with the broader
  Wardian theme tokens.
- **Neutral**: Users who prefer alternate developer fonts can still choose them
  through Settings without changing the shared default.
