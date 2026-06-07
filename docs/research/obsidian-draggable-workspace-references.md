# Obsidian Draggable Workspace References

This document maps Obsidian's draggable workspace and pane model to design
patterns relevant to Wardian's malleable surfaces, plugin-hosted views, and
local-first workspace state.

This is not an endorsement, affiliation claim, product evaluation, or
competitive teardown. The notes below describe public architecture and design
pressure only.

Last reviewed: 2026-06-07.

Source basis: observations are based on `obsidianmd/obsidian-api` commit
`2e88986`, public Obsidian Help, and public Obsidian Developer Documentation.

## Context

This note studies Obsidian as a reference for malleable, modular GUIs. The
focus is not Obsidian's note model; it is how the app makes panes, tabs,
sidebars, pop-out windows, plugin views, and dragged content feel rearrangeable
without making the user think about the underlying layout graph.

Obsidian's core application source is not public. This breakdown is grounded in
public Obsidian help, official API type definitions, and developer docs. Where
the exact private drag manager implementation is not exposed, this note labels
the point as an implementation inference from the public API contract and DOM
behavior.

## Core Pattern

Obsidian models the interface as a nested workspace tree:

```text
Workspace
  -> rootSplit / leftSplit / rightSplit / workspace windows
    -> WorkspaceSplit / WorkspaceSidedock / WorkspaceRoot
      -> WorkspaceTabs
        -> WorkspaceLeaf
          -> View
```

The smoothness comes from keeping drag and layout operations at the workspace
tree level. A drag is not primarily "move this DOM subtree"; it is "move this
leaf/view state to another tab group, split, sidebar, or window, then let the
workspace render the new tree".

## Feature-By-Feature Breakdown

### 1. Workspace tree as the single layout model

The public `Workspace` API exposes `rootSplit`, `leftSplit`, `rightSplit`,
`containerEl`, layout readiness, layout load/change APIs, and layout save
debouncing. The public item classes expose `WorkspaceRoot`, `WorkspaceSplit`,
`WorkspaceSidedock`, `WorkspaceTabs`, `WorkspaceLeaf`, and `WorkspaceWindow`.

Implementation effect:

- Main content, sidebars, and pop-out windows are variations of the same
  workspace item graph.
- Dragging a tab between main area and sidebar is a tree mutation, not a
  separate feature bolted onto sidebars.
- Plugins can reason about "leaves" instead of special-casing every UI region.

Sources:

- `obsidian.d.ts`: `Workspace` root and side splits, layout APIs, and item
  classes.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L7654-L7815>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8108-L8361>
- <https://obsidian.md/help/workspace>

### 2. Leaf/view separation

A `WorkspaceLeaf` is the layout slot. A `View` is the content inside that slot.
The public API exposes `WorkspaceLeaf.view`, `openFile()`, `open()`,
`getViewState()`, and `setViewState()`.

Implementation effect:

- Layout can move a leaf without requiring each view to understand drag and
  split mechanics.
- View state can be serialized, restored, or transferred independently from
  screen position.
- Custom views, markdown views, graph views, backlinks, and file explorer views
  can all live in the same leaf system.

Sources:

- `WorkspaceLeaf` parent/view/state APIs.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8136-L8193>
- <https://obsidian-developer-docs.pages.dev/Reference/TypeScript-API/WorkspaceLeaf/>

### 3. Tab groups as the desktop leaf parent

On desktop, the public API states that a `WorkspaceLeaf` is always a child of
`WorkspaceTabs`. On mobile, it may instead be a child of `WorkspaceMobileDrawer`.
This is a key abstraction: the draggable unit is a leaf, but the local tab strip
is a `WorkspaceTabs` parent.

Implementation effect:

- Reordering tabs within a group and moving tabs between groups use the same
  parent/child relationship.
- The desktop and mobile shells can differ without changing the leaf/view
  contract.
- Drag behavior can be implemented at the tab-group boundary instead of inside
  every view.

Sources:

- `WorkspaceLeaf.parent` and `WorkspaceTabs`.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8136-L8148>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8340-L8348>

### 4. Drag/drop uses visible drop zones, not invisible guesses

Obsidian Help documents the user-facing drag behavior: tabs can be rearranged
within a tab group, moved to another tab group, used to create a new tab group,
dragged into sidebars, or dragged out to a pop-out window. During a drag,
drop zones become highlighted, and the highlighted zone determines where the tab
will be inserted.

Implementation effect:

- The user receives a concrete preview of the layout mutation before dropping.
- The same gesture covers reorder, split, sidebar placement, and pop-out
  creation.
- The interaction model is spatial: bottom creates a split, tab-strip drop
  inserts into a group, outside the window creates a window.

Sources:

- Obsidian Help: tabs, arrange tabs, drop zones, split tab groups, pop-out
  windows.
- <https://obsidian.md/help/tabs>
- <https://obsidian.md/help/drag-and-drop>

### 5. Split creation is an API operation, not only a pointer trick

The public `Workspace` API includes `createLeafBySplit()`, `splitActiveLeaf()`,
and `getLeaf('split', direction)`. The docs describe vertical splits appearing
to the right and horizontal splits appearing below the current leaf.

Implementation effect:

- Manual split commands, drag-created splits, and plugin-created splits can
  converge on the same layout operation.
- Drag behavior does not need to directly own all layout manipulation logic.
- Plugins can create layouts that feel native because they use native leaf
  operations.

Sources:

- `Workspace.createLeafBySplit()`, `splitActiveLeaf()`, and `getLeaf('split')`.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L7743-L7800>
- <https://obsidian-developer-docs.pages.dev/Reference/TypeScript-API/Workspace/>

### 6. Resize is treated as workspace and view lifecycle

Obsidian Help documents dragging tab-group edges and sidebar edges to resize.
The public API emits a `resize` event when a `WorkspaceItem` is resized or the
workspace layout changes. `View.onResize()` is part of the view lifecycle. The
developer CSS reference exposes divider and resize-handle variables such as
divider color, hover color, width, hover width, and vertical height.

Implementation effect:

- Resize is not only visual; views get lifecycle notification to reflow their
  content.
- Theme authors can tune resize affordance visibility without rewriting
  behavior.
- Smoothness comes from separating divider interaction, layout mutation, and
  view reflow.

Sources:

- Obsidian Help: resize tab groups.
- `Workspace.on('resize')` and `View.onResize()`.
- CSS variables for dividers and resize handles.
- <https://obsidian.md/help/tabs>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L7966-L7972>
- <https://obsidian-developer-docs.pages.dev/Reference/TypeScript-API/ItemView/>
- <https://obsidian-developer-docs.pages.dev/Reference/CSS-variables/Window/Divider>

### 7. Deferred leaves keep many tabs cheap

`WorkspaceLeaf.isDeferred` is public. The API states that a deferred leaf in the
background has a `DeferredView` instead of the real view type, such as
`MarkdownView`, and `loadIfDeferred()` loads it when needed.

Implementation effect:

- Obsidian can keep many tabs and saved layouts around without fully loading
  every view.
- Revealing a leaf can explicitly load it, which prevents plugins from racing
  against unloaded content.
- Background panes feel persistent without requiring all content to be live.

Sources:

- `WorkspaceLeaf.isDeferred` and `loadIfDeferred()`.
- `Workspace.revealLeaf()` notes that awaiting ensures the view is loaded and
  not deferred.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8180-L8193>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L7931-L7937>

### 8. Layout persistence is explicit and debounced

The public API exposes `getLayout()`, `changeLayout()`, and
`requestSaveLayout`, described as a debouncer that saves the current workspace
layout. Obsidian Help says tab arrangements persist until the next app open, and
the Workspaces core plugin saves open files, tabs, sidebar widths, and sidebar
visibility.

Implementation inference:

- Drag and resize operations likely update an in-memory workspace tree
  immediately, then schedule persistence through `requestSaveLayout` instead of
  writing layout state on every pointer movement.
- This is the right shape for smooth drag: immediate visual mutation, deferred
  persistence.

Sources:

- `Workspace.getLayout()`, `changeLayout()`, `requestSaveLayout`.
- Obsidian Help: tabs and Workspaces core plugin.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L7710-L7741>
- <https://obsidian.md/help/tabs>
- <https://obsidian.md/help/Plugins/Workspaces>

### 9. Stacked tabs are a mode of tab groups, not a separate view

Obsidian Help describes stacked tabs as tabs that slide over other tabs in the
same tab group, inspired by Andy Matuschak's sliding notes. The developer CSS
reference exposes stacked-tab variables for pane width, header width, font,
text alignment, text transform, text writing mode, and shadow.

Implementation effect:

- "Sliding panes" reuse the same tab group and leaf model.
- Stacked mode is largely presentation and geometry over existing tab content,
  not a second navigation system.
- Theme-level variables let the visual treatment be tuned without changing the
  workspace model.

Sources:

- Obsidian Help: stacked tabs.
- Developer CSS variables for tab stacks.
- <https://obsidian.md/help/tabs>
- <https://obsidian-developer-docs.pages.dev/Reference/CSS-variables/Components/Tabs>

### 10. Pop-out windows preserve the workspace graph

The public API includes `moveLeafToPopout()` and `openPopoutLeaf()` on desktop.
`WorkspaceItem.getContainer()` returns either `WorkspaceRoot` or
`WorkspaceWindow`, and `WorkspaceWindow` has its own `win` and `doc`.

Implementation effect:

- Moving a tab into a window is still a leaf/container operation.
- Window-specific DOM state is represented explicitly instead of assuming a
  single global document.
- Plugins and views can operate across pop-outs using workspace/window handles.

Sources:

- `Workspace.moveLeafToPopout()` and `openPopoutLeaf()`.
- `WorkspaceItem.getContainer()` and `WorkspaceWindow`.
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L7802-L7815>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8121-L8129>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8350-L8361>

### 11. Plugin views join the same draggable surface system

Developer docs instruct plugins to create custom views by extending `ItemView`,
then register the view type with `Plugin.registerView(type, viewCreator)`.
Activation can reuse an existing leaf from `getLeavesOfType()`, create a side
leaf with `getRightLeaf(false)`, set its view state, and call `revealLeaf()`.

The docs explicitly warn plugin authors not to manage persistent references to
view instances because Obsidian can call the view factory multiple times.

Implementation effect:

- Plugin surfaces inherit native placement, dragging, tab grouping, sidebar
  behavior, and pop-out behavior because they are normal views in leaves.
- Obsidian remains malleable without plugin authors owning layout mechanics.
- Factory-based view creation prevents stale view references when leaves move,
  close, defer, or restore.

Sources:

- Developer docs: custom views, `ItemView`, `registerView`, `getLeavesOfType`,
  `setViewState`, and `revealLeaf`.
- `Plugin.registerView()` API.
- <https://obsidian-developer-docs.pages.dev/Plugins/User-interface/Views>
- <https://obsidian-developer-docs.pages.dev/Reference/TypeScript-API/Plugin/registerView>
- <https://obsidian-developer-docs.pages.dev/Reference/TypeScript-API/ItemView/>

### 12. Drag sources are normalized beyond tabs

Obsidian Help documents drag sources including files from the file explorer,
search results, backlinks, unlinked references, links inside notes, external
HTML, and native filesystem files. Drop destinations include tab headers,
folders, editors, and bookmarks.

Implementation effect:

- Dragging is a platform-wide interaction vocabulary, not a tab-only feature.
- Different source types can be interpreted by destination-specific handlers.
- The editor drop event gives plugins a hook for participating in drop handling
  without owning global drag behavior.

Implementation inference:

- The public API exposes `Workspace.on('editor-drop')` and the unofficial
  reverse-engineered typings for Obsidian's internals describe a central
  `DragManager` with `Draggable`, `handleDrag()`, `handleDrop()`, overlay,
  hover, ghost, and source tracking. That shape matches the public behavior,
  but it is not an official API commitment.

Sources:

- Obsidian Help: drag sources and destinations.
- `Workspace.on('editor-drop')`.
- Unofficial internal typing reference for `DragManager`.
- <https://obsidian.md/help/drag-and-drop>
- <https://github.com/obsidianmd/obsidian-api/blob/2e88986/obsidian.d.ts#L8053-L8059>
- <https://fevol.github.io/obsidian-typings/api/obsidian-typings/namespaces/internals/interfaces/dragmanager/>

## Why The Draggable UI Feels Smooth

The smoothness is not just animation. It is structural:

1. The draggable object is usually a `WorkspaceLeaf`, a stable unit with a view
   and view state.
2. Drop zones preview legal mutations before the user releases the pointer.
3. Splits, tabs, sidebars, and windows are one workspace tree, not unrelated
   containers.
4. Background leaves can defer real view loading.
5. Layout persistence is debounced.
6. Views receive lifecycle events for open, close, resize, state, and unload.
7. Plugin views enter through the same view/leaf contract as core views.
8. CSS variables expose visual affordances without exposing private behavior.

## Wardian Implications

Wardian should treat "draggable components" as a workspace-model problem before
treating it as a drag-library problem.

Recommended direction:

1. Define a stable `Surface` or `PaneLeaf` model with view state separate from
   layout position.
2. Use one layout tree for main area, sidebars, pop-outs, terminal panes, file
   panes, browser panes, agent feeds, logs, and future plugin views.
3. Make drag operations mutate the layout tree, then render from the tree.
4. Use visible drop zones for all legal placement outcomes: reorder, split,
   sidebar, pop-out, and tab group insertion.
5. Keep view instances lifecycle-managed by the host; plugins should register
   factories and query leaves rather than owning long-lived view references.
6. Defer or hibernate background surfaces where possible, especially terminals,
   browser previews, and expensive agent/session views.
7. Debounce layout persistence after drag and resize changes.
8. Expose theme variables for affordances such as drop zones, split dividers,
   tab headers, stacked tabs, and active/focused surfaces.

## Design Principle

Obsidian's malleability comes from making workspace structure the primitive.
Dragging feels smooth because the app is rearranging durable leaves in a common
tree, not asking every component to become its own draggable mini-application.
