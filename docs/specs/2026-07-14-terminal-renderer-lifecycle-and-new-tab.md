# Terminal Renderer Lifecycle and New Tab Surface

## Decision

The Agents surface must not equate scroll intersection with terminal lifetime. A terminal presentation owns a continuously updated headless parser and may own an xterm renderer. Scrolling may change WebGL priority, but it must not repeatedly destroy and reconstruct xterm while the Agents surface remains open.

Renderer retirement is an ownership transition. Once a renderer is retired, new output must not acquire it, in-flight renderer operations must finish before physical disposal, and post-await work must only run for the same current renderer generation. This removes the initialization crash caused by a broker snapshot resuming after `entry.renderer` was cleared.

For up to the renderer budget, all terminal cards in the visible Agents surface remain resident. Above the budget, residency uses a stable window: cards are retained until capacity is needed for an approaching card, rather than suspended immediately when they leave the intersection margin. Physical intersection controls WebGL promotion independently.

Auto layout scores the number of useful agent cards that fit in the viewport, not whether the entire roster can be compressed into one screen. When at least two floor-sized cards fit side by side, Auto uses a multi-column grid and allows additional rows to scroll vertically. Narrow panes stack the complete roster into a one-column scrolling grid; only an explicit Single selection hides the other agents. Temporary focus is therefore separate from responsive layout: entering Single remembers the last explicit multi-agent mode (`auto` or `grid`), and leaving focus always restores a visible multi-agent roster.

Wardian also relies on xterm's own repaint after `scrollLines`; it must not issue a second full-grid refresh for every wheel event.

## New Tab Contract

The default `+` action creates a real **New Tab** in the clicked pane. Its content is the existing full-pane visual surface picker with surface icons, titles, descriptions, Browse all, and recently closed affordances. It is not a modal.

Choosing a surface converts the New Tab in place, preserving its tab position and pane. If the requested singleton is already open, Wardian removes the placeholder and focuses the existing surface instead of creating a duplicate. The optional setting that maps `+` directly to the searchable palette remains supported; Quick Open and the command palette remain transient overlays.

An empty pane continues to derive the same picker without persisting a placeholder. A user-created New Tab is a canonical registered surface so Dockview and safe-mode projection share one active-tab and ordering model.

## Pane Split Admission

Every split affordance uses the destination pane's live geometry. Wardian advertises a 50/50 edge preview only when both resulting panes can satisfy the same minimum width or height enforced by Dockview group constraints. Impossible edge targets show no edge preview and commit no split; center moves remain available. The preview, pointer-drop commit, pane menu, and keyboard split paths share this predicate so a resize during drag cannot create a layout the preview could not promise.

## Verification

- A delayed xterm write can overlap renderer retirement without a null dereference or operation on a disposed terminal.
- Scrolling an Agents grid of at most 24 terminals preserves renderer identity and does not request replacement snapshots.
- Larger grids use bounded, stable residency and prefer newly approaching cards without continuous boundary churn.
- Auto shows the complete roster in a scrolling grid, selecting multiple columns when useful cards fit and one column otherwise; explicit focus exits to the remembered Auto/Grid mode.
- Wheel scrolling performs one xterm-managed full repaint per input.
- `+` creates an inline New Tab in the requested pane; choosing a card replaces it at the same surface id and index.
- The palette preference and keyboard Quick Open behavior remain unchanged.
- Narrow destinations reject edge previews and split commands while retaining center moves; viable destinations preserve exact half-pane previews.
- Native PTY coverage validates the lifecycle in the Tauri runtime; browser coverage validates the tab and picker interaction.
