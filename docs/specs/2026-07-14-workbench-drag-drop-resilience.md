# Workbench Drag-and-Drop Resilience

**Status:** Implemented and browser-verified
**Issue:** #513
**PR:** #667

## Problem

Workbench drag-and-drop currently lets Dockview mutate its transient group tree while Wardian separately mutates the persisted workbench tree. During an edge drop, Wardian can create a canonical group that Dockview removes while it is still empty. Reconciliation then throws `Dockview group projection failed`, replacing the application with the fatal error boundary. The same split ownership produces misleading drop previews and leaves empty panes behind.

The tab-strip `+` action also opens the dense searchable surface list. The intended default is the visual surface chooser used by the empty-workbench home, while retaining the searchable list as an explicit preference.

## Interaction Contract

- Reordering tabs inside a pane never changes pane geometry.
- Moving a tab to another pane is one canonical transaction.
- Dropping on a pane edge creates a 50/50 split whose preview matches the resulting pane bounds.
- If moving or closing the last tab empties a pane, that pane collapses immediately and its sibling expands into the released space.
- The final remaining pane is never removed. When it has no tabs, it renders the Home surface chooser.
- A rejected or interrupted drag restores the canonical layout without escalating to the fatal application boundary.
- The tab-strip `+` button opens the visual surface chooser by default.
- Settings may switch `+` to the searchable command-style surface list. Keyboard Quick Open remains searchable regardless of this preference.

## Architecture

Wardian's `WorkbenchDocumentV1` remains the only durable layout model. Dockview remains the renderer and pointer-drag engine, but it may not independently commit pane topology. Center/tab-strip drops may use Dockview's native movement only when the destination group already exists. Edge drops are intercepted and committed as one Wardian batch (`split_group`, then `move_surface`).

The model collapses an emptied source group as part of cross-group `move_surface`. This makes automatic pane collapse deterministic for pointer, keyboard, menu, and restored-layout paths. The adapter treats a missing transient group as recoverable: it rebuilds or schedules canonical reconciliation rather than throwing.

Drop-preview geometry is derived from the actual destination group rectangle. Center drops cover the content area; edge drops cover exactly one half of the content area. Header height and sash thickness are excluded, so the preview describes the resulting pane rather than a smaller nested box.

The visual chooser and searchable list remain separate presentations over the same surface registry. A shared launcher-mode setting selects which presentation the `+` button opens; Quick Open and Command Palette commands keep their current semantics.

## Alternatives Considered

1. **Canonical Wardian transaction with a recoverable Dockview projection — selected.** Preserves Markdown/disk-inspectable state, keyboard parity, and stable persistence while fixing the race at its source.
2. **Persist Dockview JSON as the layout authority.** Rejected because it couples durable state to library-private schema and violates the existing workbench model boundary.
3. **Disable pane-edge dragging and require split commands.** Rejected because it removes a core tactile interaction and still leaves center-move empty-pane behavior inconsistent.

## Error Handling

- Adapter projection must not throw for a transiently missing group.
- A failed adapter command schedules one canonical reconciliation and records a diagnostic console error with group and surface identifiers.
- Reconciliation is idempotent and bounded; it must not loop when Dockview removes an empty group.
- Persisted documents containing empty non-root groups are normalized by subsequent accepted moves or explicit group close, without deleting surfaces.

## Settings

Add `workbench_new_tab_action` with values:

- `home` (default): open the visual app/surface chooser.
- `palette`: open the searchable surface list.

The field participates in frontend defaults, app-setting overrides, Rust serialization, migration defaults, and Settings UI. Missing or invalid values normalize to `home`.

## Verification

- Model coverage verifies cross-pane moves that collapse an emptied source, including nested splits and preservation of the final pane.
- Adapter coverage verifies recoverable missing-group projection, serialized drag feedback, and center/edge routing without fatal exceptions.
- Host and browser coverage verify the default visual `+` chooser, its **Browse all surfaces** handoff, the persisted searchable-list preference, pane capture, and searchable Quick Open in both preference modes.
- Real-pointer browser coverage verifies in-strip reorder, center moves, sole-tab source collapse, and edge splitting. It polls canonical DOM and saved topology, rejects empty non-final groups, and records page and fatal console errors.
- The edge-preview test compares Dockview's visible selection with the live content target after both rectangles stabilize across animation frames. It passed three consecutive runs and produced `e2e/screenshots/workbench-drag-drop/2026-07-14/edge-preview.png`.
- Scoped delivery evidence is recorded in `.superpowers/sdd/workbench-e2e-docs-report.md` for commits `502f483a`, `44dd76e4`, and `a9e972a1`.
