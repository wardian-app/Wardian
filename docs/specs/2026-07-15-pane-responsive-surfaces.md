# Pane-Responsive Surfaces

## Context

Workbench surfaces can occupy the full workspace, one side of a split, or a nested pane. Several current layouts still respond to the desktop viewport or assume full-window width. This causes control overlap in Workflows, over-compressed agent terminals, and other partial-pane failures.

The Agents Auto layout also conflates a terminal card's hard rendering floor with its preferred working size. In a tall pane, Auto therefore packs as many 280px-high terminals as possible instead of preserving the more useful proportions of Wardian's original grid. Explicit Grid row gutters compound the problem by positioning themselves without the grid's top padding or accumulated row gaps.

## Goals

- Make every registered non-agent surface usable in a partial-width Workbench pane.
- Make responsiveness depend on the containing pane rather than the application window.
- Restore useful default terminal proportions in Agents Auto without hiding agents.
- Keep explicit Grid tactile and ensure its resize affordances remain aligned while scrolling.
- Add browser-level split-pane coverage so future surfaces cannot silently assume full-window geometry.

## Agents Layout

Agents Auto uses separate preferred and minimum card sizes:

| Card mode | Preferred size | Hard floor |
| --- | --- | --- |
| Terminal | 720 x 450px | 520 x 280px |
| Chat | 520 x 450px | 360 x 280px |

The 450px preferred height matches the existing manual Grid default. Auto ranks candidate layouts by useful card geometry near the preferred size rather than by the number of cards that can be packed at the hard floor. It adds columns when the pane can support them, keeps the complete filtered roster logically visible, and scrolls additional rows. The hard floor remains a constraint used during genuinely narrow or short layouts, not the default target.

Auto retains the existing resize hysteresis so small Dockview drag changes do not churn between shapes. Explicit Single remains the only mode that hides the rest of the roster.

Explicit Grid remains backed by the persisted column tracks and row height. Horizontal row gutters are positioned from the same geometry as the CSS grid:

`top padding + completed row heights + completed row gaps - half gutter height`

Stack-exit handles use the same row-origin calculation. This removes the cumulative upward drift currently visible on later rows.

## Surface Container Contract

Every `.wardian-workbench-surface-panel` is an inline-size CSS query container. Responsive rules target the live Dockview pane width, not viewport media queries. Surface content must retain `min-width: 0`, avoid fixed-width sibling combinations that exceed its container, and keep primary actions reachable without horizontal page scrolling.

### Workflows

- The primary toolbar wraps into ordered rows when the pane cannot contain the selector, modes, name, and actions on one line.
- Controls retain their readable sizes; they do not overlap or shrink into one another.
- The run drawer and selected-node inspector remain side-by-side in regular panes and become pane-local overlay drawers in compact panes.
- The builder canvas remains mounted and retains its current state while a compact drawer is open.
- Launch and node-library overlays clamp their padding and width to the pane.

### Graph

- The toolbar's scope, lens controls, and actions wrap based on pane width.
- The inspector remains a fixed side column in regular panes and becomes a right-side pane-local overlay in compact panes.
- The canvas remains mounted and continues to receive its real container geometry.

### Library

- The normal section rail, list, and resizable detail pane remain side-by-side when the pane can support them.
- In narrow panes, the section rail and list form the browse state. Selecting an item opens a full-pane detail state with a Back affordance.
- Returning to the list does not discard editor state or bypass existing dirty-content protections.

### Dashboard and New Tab

- Dashboard cards stack their fixed metadata regions based on pane width rather than Tailwind viewport breakpoints.
- New Tab's launcher and recently closed section switch to their compact arrangements through the Workbench surface container.

### Queue and Garden

Queue and Garden already size from their immediate containers. They receive regression coverage for partial-width panes, but no layout redesign unless that coverage exposes a concrete overflow or clipped-action defect.

## Verification

Unit tests cover:

- preferred-size Auto candidate selection at representative pane sizes;
- hard-floor fallback and complete-roster scrolling;
- hysteresis around layout thresholds;
- exact Grid and stack-exit gutter offsets including padding and accumulated gaps;
- compact/regular presentation state where JavaScript behavior is required, such as Library detail navigation.

Browser tests build a real Workbench split document and open each non-agent surface in a partial-width group. They assert that surface roots do not horizontally overflow, primary controls do not overlap, compact drawers remain closable, and canvas-based surfaces receive non-zero pane geometry. Representative screenshots cover Agents Auto, a later Grid gutter while scrolled, and compact Workflows.

Native tests remain reserved for terminal renderer fitting and PTY geometry. The layout algorithm and DOM affordances are proven at unit and browser layers; a native smoke confirms that the changed Agents Auto geometry still fits live terminal renderers without initialization or ownership regressions.
