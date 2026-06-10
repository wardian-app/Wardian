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
| `--color-wardian-bg` | `#040804` (green-black) | `#1f1f1f` | VS Code editor background; lifts off pure black |
| `--color-wardian-card` | `#0a120a` | `#262626` | Clear card/bg separation on the neutral ramp |
| `--color-wardian-sidebar-primary` | `#111827` (blue) | `#181818` | Darker than bg, like VS Code activity bar |
| `--color-wardian-sidebar-secondary` | `#0f172a` (blue) | `#1c1c1c` | Same family as primary sidebar |
| `--color-wardian-text` | `#e8f2e8` (green) | `#e6e6e6` | Neutral, ~12:1 contrast on `#1f1f1f` |
| `--color-wardian-text-bright` | `#d1d5db` | `#cfcfcf` | Neutralized |
| `--color-wardian-text-muted` | `#88a088` (green) | `#a8a8a8` | Neutral, ~6.5:1 contrast |
| `--color-wardian-text-muted-neutral` | `#9ca3af` (blue) | `#9b9b9b` | Neutralized |
| `--color-wardian-border` | `#1b261b` (green) | `#2e2e2e` | Visible seams on neutral surfaces |
| `--color-wardian-border-heavy` | `#2d2d44` (blue) | `#404040` | Neutralized, higher visibility |
| `--color-wardian-border-light` | `#374151` (blue) | `#383838` | Neutralized |
| `--color-wardian-accent-hover` | `#b8860b` (darker) | `#e3c04f` | Hover should brighten, not darken, in dark mode |
| `--color-wardian-off` | `#4b5563` | `#6e7681` | Off-status dots were nearly invisible on dark gray |
| `--color-wardian-input-bg` | `rgba(31,41,55,…)` (blue) | `rgba(50,50,50,…)` | Neutralized |
| `--color-wardian-card-bg-muted` | `rgba(31,41,55,0.3)` | `rgba(255,255,255,0.05)` | Neutral hover/elevation wash |

Companion changes outside the CSS variable layer:

- xterm `DARK_TERM_THEME` (`AgentTerminal.tsx`, `UserTerminalPanel.tsx`):
  background `#020402` → `#1f1f1f`, foreground `#EEF2EE` → `#e6e6e6`,
  selection `#1E261E` → `#3a3a3a`. Cursor stays gold.
- `index.html` `theme-color` meta `#0b0d10` → `#1f1f1f`.
- The shared `<select>` chevron SVG stroke `#88a088` → `#909090` (neutral in
  both themes).

## Non-Goals

- Light mode tokens are untouched.
- Semantic status colors (emerald/cyan/amber/red) are unchanged except `off`.
- No component-level class changes; everything flows through the existing
  variable layer per the semantic theming rule.
