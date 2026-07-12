# Agents Workbench UX Repair Design

## Goal

Restore Wardian's original useful multi-agent grid while retaining the new workbench's tabs and splits, and make the window chrome behave like Obsidian's frameless desktop interface.

## Window chrome and tabs

Wardian will use the existing undecorated Tauri window, but the React shell will no longer render a separate empty titlebar above Dockview. The workbench occupies the main column from the top of the window, so every Dockview group touching the top edge supplies that segment of the window chrome. Groups created below another group retain their local headers.

The left sidebar controls and right window controls remain stable chrome regions. Dockview continues to own tab DOM, drag and drop, overflow, focus, and ARIA semantics. Empty top-edge header space is a native window drag region; tabs and controls are not. Double-clicking empty top chrome toggles maximize.

Tabs are compact, flat surfaces. The active tab connects visually to its content; close appears on the active or hovered tab. The New Surface plus action appears immediately after the last visible tab through Dockview's after-tabs action slot. Overflow and pane actions stay at the far edge. The top row remains 36 pixels high.

An empty group is a launcher, not a placeholder. It presents the registered core surfaces as a responsive tool grid with a recognizable line icon, title, and one-line purpose, followed by recently closed work. Selecting a tool opens it directly in that group; the full surface dialog remains available for resource-backed and future contributions.

## Agents surface

All user-facing `Agents Overview` copy becomes `Agents`. The internal `agents-overview` surface type remains unchanged so persisted workbenches continue to load.

The original grid's persisted column tracks, row height, resize handles, spacing, and card proportions are the grid foundation:

- `auto` shows the original-style multi-column grid when cards have useful bounds. If the surface becomes too small, it shows the focused agent as a singleton. It never chooses a one-column multi-agent stack of short horizontal cards.
- `grid` honors the user's columns and row sizing, even when the surface is narrow.
- `single` gives the focused agent the entire surface.

Maximize enters `single` and restore returns to the previous `auto` or `grid` mode. Mode and focused agent remain surface-local and persisted.

## Terminal rendering and geometry

`Activate terminal renderer` is removed from the interface, tests, and guides. Renderer eviction remains an internal resource-management state.

When a visible presentation needs a renderer, Wardian waits for nonzero container bounds, restores Xterm, fits it to those bounds, and reveals it after the first fitted frame. A slow restore may show a quiet loading state. A genuine failure shows an error and Retry action.

The same reveal gate applies on first mount and when a hidden presentation becomes visible again. Stale snapshot canvases are removed before reveal so a renderer captured at another card or pane size is never briefly scaled into the current surface.

Renderer geometry is presentation-local. A `ResizeObserver` drives fitting for tab changes, pane resizing, grid resizing, sidebar changes, and maximize/restore. Only the interactive owner submits rows and columns to the PTY. Mirrors render normally but cannot resize the PTY. Remote and desktop presentations therefore cannot corrupt one another's geometry.

The terminal component will separate renderer lifecycle from geometry/PTY-resize policy so eviction, restoration, ownership, and fitting no longer share one tangled UI branch.

## Persistence and recovery

Existing workbench documents and surface state load without a destructive reset. The public title changes to `Agents`, while internal identifiers remain stable. Invalid documents continue through the existing safe-layout and recovery boundaries.

## Verification

The repair requires:

- Unit coverage for titlebar action placement, tab appearance contracts, original grid tracks, auto grid-to-single transitions, renderer restoration, and owner-only PTY resizing.
- Browser E2E at wide and narrow viewports for fused top chrome, adjacent plus action, overflow, side-by-side and downward splits, and Agents layout transitions.
- Native E2E for real Xterm fitting, PTY resize behavior, eviction/restoration, and ownership transfer.
- Feature-specific screenshots of the restored Agents grid, fused tabs, side-by-side top groups, and a downward split.
- Full frontend and backend validation required by `AGENTS.md`.

## Acceptance criteria

- No separate tab bar appears below the window chrome.
- New Surface plus is immediately after the tabs.
- An empty group offers a themed, keyboard-accessible surface launcher instead of raw placeholder controls.
- Top-edge split groups participate in the titlebar; lower groups retain local headers.
- The surface is labeled `Agents` everywhere users see it.
- Wide Agents surfaces reproduce the original broad multi-agent grid.
- Auto never renders a one-column multi-agent strip stack.
- `Activate terminal renderer` is absent from source, tests, documentation, and runtime UI.
- Visible reclaimed terminals restore automatically and fit before display.
- Initial and returning terminals never reveal a stale downscaled snapshot before their first fitted frame.
- Only the current interaction owner resizes the PTY.
