# VS Code Workbench Layout References

This document maps VS Code's workbench layout model to design patterns relevant
to Wardian's constrained modular workspaces, editor-like agent surfaces, and
extension-safe placement zones.

This is not an endorsement, affiliation claim, product evaluation, or
competitive teardown. The notes below describe public architecture and design
pressure only.

Last reviewed: 2026-06-07.

Source basis: observations are based on `microsoft/vscode` commit `d40db46`
and public VS Code user and extension documentation.

## Context

This note studies VS Code as a reference for modular developer workbenches. It
is a weaker fit than cmux or Obsidian for Wardian's "malleable GUI" goal,
because VS Code does not treat every primitive as a freely draggable surface.
The strongest draggable primitive is the editor tab/file. Views and panels are
also movable, but mostly within predefined workbench regions: Primary Side Bar,
Secondary Side Bar, Panel, Activity Bar, editor groups, and auxiliary windows.

That limitation is useful. VS Code shows how much leverage a product can get
from a constrained, stable workbench skeleton with strong extension contracts
and predictable placement zones.

## Core Pattern

VS Code models the UI as a workbench with fixed high-level parts:

```text
Workbench
  -> Activity Bar
  -> Primary Side Bar
  -> Editor Area / Editor Groups
  -> Panel
  -> Secondary Side Bar
  -> Status Bar
  -> Auxiliary windows
```

Inside that skeleton, the most fluid region is the editor area. Editor groups
are backed by a two-dimensional grid/split layout. Views and view containers can
move between supported workbench regions, but they do not become arbitrary
editor-like surfaces by default.

## Feature-By-Feature Breakdown

### 1. Fixed workbench skeleton

VS Code has a stable frame: Activity Bar, Side Bar, Editor, Panel, Secondary
Side Bar, and Status Bar. The user can rearrange visibility and side placement,
but the skeleton remains recognizable.

Implementation effect:

- Users always know where navigation, editing, output, and status live.
- Extensions plug into known regions rather than inventing independent shells.
- The product can support high customization without making layout unbounded.

Sources:

- VS Code User Interface docs.
- VS Code Custom Layout docs.
- <https://code.visualstudio.com/docs/editing/userinterface>
- <https://code.visualstudio.com/docs/editor/custom-layout>

### 2. Editor groups use a real grid/split widget

The editor area uses a reusable `GridView`/`Grid` implementation. `GridView`
represents layout as a tree composition of orthogonal `SplitView` instances.
Each view has DOM, min/max size constraints, layout callbacks, visibility
callbacks, and optional boundary sash handling.

Implementation effect:

- Editor splits are structural, not CSS-only.
- Resize, snap, min/max constraints, maximization, serialization, and
  deserialization are centralized.
- Dragging a tab to the side can resolve into a grid mutation, not a custom
  per-editor behavior.

Sources:

- `src/vs/base/browser/ui/grid/gridview.ts`: `IView`, `GridView`, tree
  composition, add/remove/move, deserialize.
- `src/vs/base/browser/ui/grid/grid.ts`: model-element addressing and
  `SerializableGrid`.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/base/browser/ui/grid/gridview.ts#L39-L135>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/base/browser/ui/grid/gridview.ts#L983-L1041>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/base/browser/ui/grid/gridview.ts#L1202-L1378>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/base/browser/ui/grid/grid.ts#L217-L230>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/base/browser/ui/grid/grid.ts#L480-L522>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/base/browser/ui/grid/grid.ts#L779-L847>

### 3. Editor tab drag/drop is deeply implemented

`MultiEditorTabsControl` registers drag/drop observers on tab containers and
individual tabs. It stores dragged editors in local transfer data, applies drag
images, updates drop feedback, supports multi-selected editor moves, computes
target indexes, moves/copies editors between groups, merges groups, accepts tree
item drops, and falls back to resource drop handling for URI/file drops.

Implementation effect:

- File/editor movement feels native because it owns all important cases:
  reorder, move, copy, multi-select, group merge, external resource drop, and
  new-window behavior.
- Drop feedback is visual and continuous, not only a final drop result.
- The editor tab is the closest thing VS Code has to Obsidian's leaf or cmux's
  surface.

Sources:

- `src/vs/workbench/browser/parts/editor/multiEditorTabsControl.ts`: tab and
  container drag/drop.
- VS Code User Interface docs: drag a file to any side of the editor region.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/browser/parts/editor/multiEditorTabsControl.ts#L345-L405>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/browser/parts/editor/multiEditorTabsControl.ts#L1087-L1215>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/browser/parts/editor/multiEditorTabsControl.ts#L2225-L2315>
- <https://code.visualstudio.com/docs/editing/userinterface>

### 4. Drag data is normalized before destinations interpret it

The platform drag/drop layer defines VS Code-specific data transfer keys such
as `CodeEditors` and `CodeFiles`. It extracts editor drop data, external files,
raw resource data, and registered drag/drop contributions. A contribution
registry lets handlers provide editor inputs or handle dropped resources.

Implementation effect:

- Editor destinations do not parse every possible drag source from scratch.
- External files, internal editors, tree items, symbols, markers, notebooks,
  and contribution-provided data can converge into editor inputs.
- This is useful for Wardian: normalize dropped things into typed open intents
  before mutating layout.

Sources:

- `src/vs/platform/dnd/browser/dnd.ts`: `CodeDataTransfers`,
  `extractEditorsDropData`, `extractEditorsAndFilesDropData`, and contribution
  registry.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/platform/dnd/browser/dnd.ts#L32-L63>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/platform/dnd/browser/dnd.ts#L141-L165>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/platform/dnd/browser/dnd.ts#L322-L398>

### 5. View containers are movable, but within defined regions

VS Code views can live in the Side Bar or Panel, and users can move views to
another view container such as the Secondary Side Bar. Extension docs describe
views as content containers that can contain Tree Views, Welcome Views, or
Webview Views. They can be rearranged or moved to another view container.

Implementation effect:

- Extension views get user-level mobility without becoming arbitrary editor
  surfaces.
- The movable unit is a view descriptor or view container, not an unconstrained
  custom component.
- This is a constrained but stable compromise between flexibility and
  consistency.

Sources:

- VS Code Views UX guidelines.
- VS Code Tree View API.
- VS Code Custom Layout docs.
- <https://code.visualstudio.com/api/ux-guidelines/views>
- <https://code.visualstudio.com/api/extension-guides/tree-view>
- <https://code.visualstudio.com/docs/editor/custom-layout>

### 6. View movement is mediated by a descriptor service

`ViewDescriptorService` owns operations such as moving a view container to a
location, moving a view to a location, moving views to a container, generating
temporary user view containers, firing location/container change events, and
saving customizations.

Implementation effect:

- Drag/drop and commands share the same view movement authority.
- Custom view placement persists as view customizations rather than ad hoc DOM
  state.
- Generated containers allow a lone moved view to become its own container while
  still staying in the workbench model.

Sources:

- `src/vs/workbench/services/views/browser/viewDescriptorService.ts`:
  `moveViewContainerToLocation`, `moveViewToLocation`,
  `moveViewsToContainer`, generated containers, and saved customizations.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/services/views/browser/viewDescriptorService.ts#L354-L397>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/services/views/browser/viewDescriptorService.ts#L490-L522>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/services/views/browser/viewDescriptorService.ts#L554-L572>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/services/views/browser/viewDescriptorService.ts#L659-L722>

### 7. View drop overlays communicate legal moves

`ViewPaneContainer` has a `ViewPaneDropOverlay` that creates a
`monaco-pane-drop-overlay`, marks a pane as dragged-over, colors the overlay
based on whether the target is the Panel or Side Bar, and disposes itself after
drag leave/drop. The container registers draggable views and drop targets,
checks whether views can move, rejects invalid targets, and calls
`moveViewsToContainer(..., 'dnd')` on drop.

Implementation effect:

- View movement is visibly bounded by legal target regions.
- A view with `canMoveView === false` cannot accidentally become draggable.
- Drag affordance follows a policy layer, not only the presence of DOM handles.

Sources:

- `src/vs/workbench/browser/parts/views/viewPaneContainer.ts`: drop overlay,
  target registration, draggable view registration, and move-on-drop.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/browser/parts/views/viewPaneContainer.ts#L71-L137>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/browser/parts/views/viewPaneContainer.ts#L402-L501>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/browser/parts/views/viewPaneContainer.ts#L877-L948>

### 8. Extension views are declarative contribution points

Extensions contribute views through `contributes.views` and view containers
through `contributes.viewsContainers`. Tree views use a `TreeDataProvider`.
Webview views are available but discouraged for cases where native tree/list UI
is sufficient.

Implementation effect:

- Extension UI starts from metadata and activation events rather than raw DOM
  injection into the shell.
- VS Code controls placement, title actions, context menus, visibility, icons,
  and movement.
- Extensions are modular because the workbench owns chrome and lifecycle.

Sources:

- Extending Workbench docs.
- Tree View API docs.
- Views UX guidelines.
- Contribution Points docs.
- <https://code.visualstudio.com/api/extension-capabilities/extending-workbench>
- <https://code.visualstudio.com/api/extension-guides/tree-view>
- <https://code.visualstudio.com/api/ux-guidelines/views>
- <https://code.visualstudio.com/api/references/contribution-points>

### 9. Webviews are powerful but intentionally fenced

VS Code uses webviews for custom editor and view content, but they run inside
iframe-like boundaries. The source has a `WebviewWindowDragMonitor` because
webviews can eat drag events; VS Code disables pointer events during workbench
dragging and re-dispatches drag events to keep editor drag/drop working.

Implementation effect:

- Rich custom UI is possible, but it does not automatically become a first-class
  draggable workbench primitive.
- The host has to defend global drag/drop semantics from embedded iframe
  behavior.
- For Wardian, this is a warning: arbitrary iframe/webview surfaces need host
  mediation for drag, focus, keyboard, and lifecycle.

Sources:

- `src/vs/workbench/contrib/webview/browser/webviewWindowDragMonitor.ts`.
- `src/vs/workbench/contrib/webview/browser/webviewElement.ts`.
- Webview API docs.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/webview/browser/webviewWindowDragMonitor.ts#L11-L17>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/webview/browser/webviewElement.ts#L559-L565>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/webview/browser/webviewElement.ts#L742-L761>
- <https://code.visualstudio.com/api/extension-guides/webview>

### 10. Terminals can be panel views or editor inputs

VS Code's integrated terminal normally lives in the Panel, but terminal support
also registers terminal editors. The source includes a `TerminalEditorInput`,
`TerminalEditor`, `TerminalEditorService`, and commands for creating terminals
in the editor area or to the side.

Implementation effect:

- A terminal can participate in editor-group placement when represented as an
  editor input.
- The panel terminal and editor terminal are related but not identical surface
  concepts.
- This is relevant to Wardian: making a primitive movable often means adapting
  it to the target region's model, not simply dragging the same component
  everywhere.

Sources:

- Terminal contribution/source references in `src/vs/workbench/contrib/terminal`.
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/terminal/browser/terminalEditorInput.ts>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/terminal/browser/terminalEditor.ts>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/terminal/browser/terminalEditorService.ts>
- <https://github.com/microsoft/vscode/blob/d40db46/src/vs/workbench/contrib/terminal/browser/terminal.contribution.ts>

## Where VS Code Is Weaker For Wardian

VS Code is not a general "drag any object anywhere" model.

Limitations:

- The most fluid object is an editor/file tab.
- Views move between predefined workbench containers, not arbitrary grid cells.
- Extension-contributed views are usually sidebar/panel views, not editor-like
  surfaces unless implemented through editor APIs.
- Webviews are fenced and require host workarounds for drag/focus behavior.
- Terminal, webview, custom editor, tree view, and panel primitives do not all
  share one universal surface contract.

This makes VS Code less directly aligned with Wardian's multi-agent surface
goal than cmux or Obsidian. Its value is discipline: a stable workbench shell,
clear contribution points, central layout services, typed drag data, and
policy-mediated movement.

## Wardian Implications

Recommended takeaways:

1. Keep a stable high-level app skeleton so the workspace remains learnable.
2. Use a real grid/split model for movable primary surfaces.
3. Normalize drag sources into typed open/move intents before destinations act.
4. Put movement authority in services, not component-local DOM code.
5. Use visible overlays for legal drop targets and policy checks for invalid
   moves.
6. Treat iframe/webview surfaces as dangerous for global drag/focus unless the
   host actively mediates them.
7. Let extension/plugin surfaces enter through declared metadata and factories,
   with host-owned chrome and lifecycle.
8. Do not copy VS Code's constraint too literally: Wardian likely needs a more
   universal surface model than VS Code provides.

## Design Principle

VS Code's workbench is smooth because it is constrained. It gives users strong
movement where it matters most, then keeps sidebars, panels, views, terminals,
and webviews inside predictable container contracts. Wardian should borrow the
service boundaries and typed drag/drop discipline, but aim for a broader
surface contract than VS Code exposes.
