# Responsive Terminal Width

- **Status:** Implemented
- **Date:** 2026-04-19

## Context and Problem Statement

On physically small displays (laptops at 1080p, fullscreen Wardian window), the
left content sidebar (~260px) and right `AgentWatchlist` (~260px) consume ~30%
of horizontal space, leaving the central grid pane starved. With one or two
agents in `GridView`, individual terminal cells fall well below comfortable
reading width. The same window dimensions on a physically larger 1080p desktop
display look fine — so width-media-query approaches do not help, since CSS
pixel dimensions are identical and no reliable physical-inches API is
available.

Two separable user pains:

1. **Sidebar real estate is fixed.** A user on a small display has no way to
   reclaim sidebar pixels short of fully collapsing them, which throws away
   navigation entirely.
2. **No focus gesture for a single terminal in grid view.** `maximizedAgentId`
   exists but hides siblings entirely; there is no intermediate "expand this
   one to fill width but keep others visible" option.

## Proposed Decision

Two complementary mechanisms, both built on existing layout primitives.

### Lever A: Resizable sidebars

Replace the static CSS variables `--sidebar-content-width` and
`--sidebar-secondary-width` (defined in `src/styles/App.css:41`) with values
written at runtime from a new Zustand store. Add a 4px hover-fattened drag
handle on the inner edge of each sidebar (`SidebarContentPane`,
`AgentWatchlist`).

Constraints:

- **Min width**: 200px (below this the sidebar contents reflow badly).
- **Max width**: 40% of current viewport width (prevents pathological drags).
- **Double-click handle**: resets that sidebar to its default (260px).
- **Collapse toggles unchanged**: existing `leftCollapsed` / `rightCollapsed`
  still hide the sidebar entirely; resize only applies when expanded.
- **Icon rail (`SidebarIconRail`)** stays fixed-width. It is already minimal.

This mirrors the pattern in VSCode, Slack, and similar tools. No
auto-shrink-on-small-window heuristic — the user owns the tradeoff via the
drag, and the persisted width handles per-machine differences naturally.

### Lever B: User-forced stacked grid mode

`GridView.tsx:121-131` already auto-switches to `gridTemplateColumns: '1fr'`
when `windowWidth < 1000` ("mobile" mode), rendering all agents in a single
column at natural `layout.row_height`. Reuse this rendering path as a
user-controllable mode.

Trigger: `useGridResize` keeps magnetic snap weights of `[0.333, 0.5,
0.666]` (the prior `1.0` snap is removed). On release of any horizontal
drag, if any resulting track exceeds `2/3 + ε` of the container width, set
`gridStacked: true`. The `ε` (≈ 0.01) keeps the `0.666` snap as a valid
2/3-1/3 layout the user can rest on without being forced into stacked.

Behavior in stacked mode:

- All visible agents render in a single column at natural
  `layout.row_height`. No special "lead" cell, no height variation. Exactly
  the same visual mode as the existing `windowWidth < 1000` auto-stack.
- Main pane is vertically scrollable.
- Exit is gesture-driven: each stacked cell renders a right-edge resize
  handle. Dragging it inward and releasing below the entry threshold
  (`< 2/3`) restores the saved pre-stacked `column_tracks` (or `[0.5, 0.5]`
  if none) and clears `gridStacked`. There is no exit button — it
  conflicted with existing per-card toolbar affordances.
- During a stack-exit drag, the grid renders a live multi-column preview
  so the dragged cell visibly shrinks; the preview is reverted on release
  if the drag did not cross the exit threshold.
- `maximizedAgentId` (full-screen single agent) is unchanged and orthogonal.
- The auto `windowWidth < 1000` behavior is unchanged: stacked rendering is
  used if either `gridStacked` or `windowWidth < 1000` is true.

### State and persistence

Extend the **existing** `src/store/useLayoutStore.ts` (which today persists
grid `column_tracks` and `row_height` under `localStorage` key
`wardian-layout`):

```ts
interface LayoutState {
  leftSidebarWidth: number;   // px, default 260
  rightSidebarWidth: number;  // px, default 260
  gridStacked: boolean;       // user-forced single-column mode
  setLeftSidebarWidth(px: number): void;
  setRightSidebarWidth(px: number): void;
  setGridStacked(v: boolean): void;
  resetLayout(): void;
}
```

- Persisted via the existing Zustand `persist` middleware to `localStorage`
  key `wardian-layout` (no key change — Zustand persist tolerates added
  fields, missing ones fall back to defaults).
- Per-installation, not per-workspace — laptop and desktop diverge naturally.
- Setters clamp to `[200, 0.4 * window.innerWidth]` for sidebar widths.
- A single `useEffect` in `App.tsx` writes `leftSidebarWidth` /
  `rightSidebarWidth` to `document.documentElement.style` as the existing
  CSS custom properties, so no consumer of `var(--sidebar-content-width)`
  needs to change.

### File-level changes

- `src/store/useLayoutStore.ts` (extend existing slice with sidebar widths
  and `gridStacked`).
- `src/styles/App.css` — keep CSS variables, drop hard-coded defaults if
  they conflict.
- `src/views/App.tsx` — wire store → CSS variables; pass `gridStacked` to
  `GridView`.
- `src/layout/SidebarContentPane.tsx` — add inner-edge resize handle.
- `src/layout/watchlist/AgentWatchlist.tsx` — add inner-edge resize handle.
- `src/views/GridView.tsx` — track drag-past-2/3 threshold, set
  `gridStacked`; honor `gridStacked` in `gridStyle` calculation; render
  exit-stacked button.

## Consequences

- **Positive**: Solves the small-display problem with one drag, no detection
  heuristics that fail silently.
- **Positive**: Stacked mode reuses the existing mobile rendering path —
  minimal new layout code, behaviors stay coherent.
- **Positive**: All state lives in one small store; CSS-variable bridge keeps
  existing styles working unchanged.
- **Positive**: Pattern is standard (VSCode, Slack, Cursor) — no
  Wardian-specific gestures users have to learn.
- **Negative**: `localStorage` persistence is per-installation, so settings
  do not sync across machines. Acceptable — that is in fact the desired
  behavior, since the laptop and desktop want different widths.
- **Negative**: Drag-past-2/3 as a stacked-mode trigger is a discoverability
  bet. Mitigated by the preview affordance and the explicit exit button.
- **Negative**: Two ways to reach single-column rendering (auto and forced)
  means slightly more conditional logic in `GridView` — kept to a single
  boolean OR.
