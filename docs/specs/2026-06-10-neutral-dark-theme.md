# Spec: Neutral High-Contrast Dark Theme

**Date**: 2026-06-10
**Status**: Implemented

## Problem

Wardian's light mode is clean and readable, but dark mode mixed three competing
tint families on top of a near-black base:

- Green-black surfaces: background `#040804`, card `#0a120a`, border `#1b261b`.
- Green-tinted text: primary `#e8f2e8`, muted `#88a088`.
- Blue-slate structure: sidebars `#111827` / `#0f172a`, heavy border `#2d2d44`,
  inputs `rgba(31, 41, 55, …)`.

The result read as muddy and low-contrast compared to modern IDE dark themes
(VS Code Dark Modern, the Codex desktop app), and the tint mismatch between
the main view and the sidebars was visible at every panel seam.

## Decision

Rebuild the dark palette as a single **neutral gray ramp** with text contrast
at AA+ levels, keeping brand identity in the gold accent and the semantic
status colors rather than in tinted surfaces. Light mode is unchanged.

| Token | Before | After | Rationale |
|---|---|---|---|
| `--color-wardian-bg` | `#040804` (green-black) | `#191919` | Neutral editor-style background; lifts off pure black |
| `--color-wardian-card` | `#0a120a` | `#212121` | Clear card/bg separation on the neutral ramp |
| `--color-wardian-sidebar-primary` | `#111827` (blue) | `#141414` | Darker than bg, like VS Code activity bar |
| `--color-wardian-sidebar-secondary` | `#0f172a` (blue) | `#181818` | Same family as primary sidebar |
| `--color-wardian-text` | `#e8f2e8` (green) | `#ececec` | Neutral, ~13:1 contrast on `#191919` |
| `--color-wardian-text-bright` | `#d1d5db` | `#d6d6d6` | Neutralized |
| `--color-wardian-text-muted` | `#88a088` (green) | `#ababab` | Neutral, ~6.5:1 contrast |
| `--color-wardian-text-muted-neutral` | `#9ca3af` (blue) | `#9e9e9e` | Neutralized |
| `--color-wardian-border` | `#1b261b` (green) | `#2f2f2f` | Visible seams on neutral surfaces |
| `--color-wardian-border-heavy` | `#2d2d44` (blue) | `#454545` | Neutralized, higher visibility |
| `--color-wardian-border-light` | `#374151` (blue) | `#3a3a3a` | Neutralized |
| `--color-wardian-accent-hover` | `#b8860b` (darker) | `#ffd470` | Hover should brighten, not darken, in dark mode |
| `--color-wardian-off` | `#4b5563` | `#6e7681` | Off-status dots were nearly invisible on dark gray |
| `--color-wardian-input-bg` | `rgba(31,41,55,…)` (blue) | `rgba(48,48,48,…)` | Neutralized |
| `--color-wardian-card-bg-muted` | `rgba(31,41,55,0.3)` | `rgba(255,255,255,0.06)` | Neutral hover/elevation wash |

In addition, `--color-wardian-accent` moves from `#d4af37` (an olive-leaning
brass that read as "dirty" on neutral grays) to `#f2c14e`, a cleaner and more
visible gold. Light mode keeps its darker `#926a09` accent for contrast.
Hardcoded `rgba(212, 175, 55, …)` gold washes (watchlist tab, select focus
ring, placeholder glow) now derive from the accent variable via `color-mix`
so they follow each theme's accent.

Companion changes outside the CSS variable layer:

- Terminal themes moved to a shared `terminalThemes.ts` module. Dark theme:
  background `#020402` → `#1a1a1a` (slightly inset relative to the `#212121`
  card), foreground `#EEF2EE` → `#ebebeb`, selection `#1E261E` → `#3d3d3d`,
  cursor aligned with the new gold. The dark theme now also defines an
  explicit 16-color ANSI palette: TUIs like Claude Code and Antigravity
  render most body text in dim and bright-black tones, and xterm's default
  ANSI ramp left that text illegibly dark on the new background.
- Dashboard/grid agent cards get `--shadow-wardian-card` elevation (the
  token was referenced by `.graph-tooltip` but never defined; it now exists
  in both themes) so panels separate from the background without relying on
  borders alone.
- `index.html` `theme-color` meta `#0b0d10` → `#191919`.
- The shared `<select>` chevron SVG stroke `#88a088` → `#909090` (neutral in
  both themes).

## Non-Goals

- Light mode tokens are untouched (except the accent-derived washes above,
  which keep their visual weight).
- Semantic status colors (emerald/cyan/amber/red) are unchanged except `off`.
- Component markup is untouched; everything flows through the existing
  variable layer per the semantic theming rule.
