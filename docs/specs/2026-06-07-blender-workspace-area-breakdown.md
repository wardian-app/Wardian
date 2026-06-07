# Blender Workspace Area Implementation Breakdown

## Context

This note studies Blender as a reference for malleable, modular application
GUI. Blender is a strong fit for Wardian's long-term "Habitat" direction
because it makes layout feel physical without making every UI object freely
float. Users split, join, swap, maximize, duplicate, and retarget rectangular
areas. Each area hosts a specialized editor: 3D Viewport, Outliner, Properties,
Timeline, Shader Editor, Node Editor, Console, Text Editor, and so on.

Blender therefore centers on scene/object work, not terminals, markdown files,
or code files. Its dynamic primitive is the editor area. The user manipulates a
workspace made of editor slots, while the scene, active object, mode, workspace
tools, and editor context create the feeling of a coherent shared environment.

Source observations are based on:

- `blender/blender` commit `bd0823aa964`
- public Blender manual and Python API documentation available on 2026-06-07

## Core Pattern

Blender's workspace model is a persistent graph of task layouts, screens, area
maps, editor instances, and regions:

```text
WorkSpace
  -> WorkSpaceLayout
    -> bScreen
      -> ScrAreaMap
        -> ScrArea
          -> SpaceLink / SpaceType
          -> ARegion / RegionType
```

The important distinction is that Blender does not treat a panel, tree, tool,
or view as an arbitrary draggable component. It treats the area as the movable
surface, the space type as the editor implementation, and the regions as the
editor's internal chrome.

## Feature-By-Feature Breakdown

### 1. Workspaces are task-level layout containers

The manual describes workspaces as predefined window layouts made of areas that
contain editors. Blender's default workspaces are task-oriented: Layout,
Modeling, Sculpting, UV Editing, Shading, Animation, Rendering, Compositing,
Geometry Nodes, Scripting, and more.

In source, `WorkSpace` owns a list of `WorkSpaceLayout` entries. Each
`WorkSpaceLayout` wraps a `bScreen *screen` with layout-local list pointers and
a name. The workspace also stores workspace tools, optional pinned scene
behavior, object mode, ordering for workspace tabs, and workspace-level viewer
state.

Implementation effect:

- Workspace tabs are not just saved views; they are task-level environments.
- A workspace can preserve different editor arrangements for different kinds
  of work.
- Tool and mode state can be scoped to the workspace rather than living only in
  individual editors.

Sources:

- Blender Workspaces manual.
- Blender Python API `bpy.types.WorkSpace`.
- `source/blender/makesdna/DNA_workspace_types.h`: `WorkSpaceLayout`,
  `WorkSpace`, workspace tools, pinned scene, object mode, tab order.
- <https://docs.blender.org/manual/en/latest/interface/window_system/workspaces.html>
- <https://docs.blender.org/api/current/bpy.types.WorkSpace.html>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/makesdna/DNA_workspace_types.h#L127-L198>

### 2. A screen is an area map, not a pile of widgets

`bScreen` stores vertices, edges, and areas. The source keeps these fields in
sync with `ScrAreaMap`, whose core fields are `vertbase`, `edgebase`, and
`areabase`. This makes the screen layout a geometric subdivision model rather
than a general widget tree.

Implementation effect:

- Split, join, resize, and swap have a structural target: the area map.
- Layout operations can reason about shared borders and corners.
- The app avoids unbounded floating-window chaos while still feeling malleable.

Sources:

- Blender Areas manual.
- `source/blender/makesdna/DNA_screen_types.h`: `bScreen`, `ScrAreaMap`,
  `AREAMAP_FROM_SCREEN`.
- <https://docs.blender.org/manual/en/latest/interface/window_system/areas.html>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/makesdna/DNA_screen_types.h#L93-L184>

### 3. Areas are editor slots with durable local history

The manual says the Blender window is divided into rectangular areas, and areas
reserve screen space for editors such as the 3D Viewport or Outliner. The
Python API mirrors this: `Area` is an area in a subdivided screen containing an
editor, with `regions`, `spaces`, and a current editor `type`.

In source, `ScrArea` stores:

- `spacetype`, the current editor type.
- `type`, the active `SpaceType` callback table.
- `spacedata`, a list of previous/current `SpaceLink` editor states.
- `regionbase`, the regions belonging to the active editor.
- `actionzones`, the corner/edge interaction handles.

The `spacedata` detail matters. Blender can switch an area from one editor type
to another while preserving previous editor-specific state for that area. The
active editor is the head of the list, and inactive editor regions are parked
inside their `SpaceLink`.

Implementation effect:

- Changing an area from Viewport to Shader Editor and back can restore useful
  local state.
- Area identity survives editor retargeting.
- Users experience surfaces as durable places, not disposable views.

Sources:

- Blender Areas manual.
- Blender Python API `bpy.types.Area`.
- `source/blender/makesdna/DNA_screen_types.h`: `ScrArea` fields.
- <https://docs.blender.org/api/current/bpy.types.Area.html>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/makesdna/DNA_screen_types.h#L620-L673>

### 4. Editor types are registered implementations

Blender initializes editor implementations through `ED_spacetypes_init()`.
That function registers built-in space types such as Outliner, 3D Viewport,
Graph Editor, Image Editor, Node Editor, Properties, File Browser, Dope Sheet,
NLA, Text, Sequencer, Console, Preferences, Clip Editor, Topbar, Status Bar,
and Spreadsheet. It then iterates over registered `SpaceType` entries to
register operators, gizmos, and dropboxes.

Implementation effect:

- Editor behavior is pluggable behind a common `SpaceType` contract.
- Layout code can host any registered editor without knowing each editor's
  internals.
- Operators, gizmos, keymaps, listeners, and drop targets live with the editor
  type instead of being scattered through the layout manager.

Sources:

- `source/blender/editors/space_api/spacetypes.cc`: `ED_spacetypes_init`,
  `BKE_spacetypes_list()` loops.
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/space_api/spacetypes.cc#L63-L151>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/space_api/spacetypes.cc#L177-L183>

### 5. Regions compose editor chrome

The manual says every editor is divided into regions. A 3D Viewport, for
example, can have a header, main region, toolbar, sidebar, and operation panel.
The Python API exposes `Region` as a region in a subdivided screen area, with a
type, alignment, dimensions, redraw, and UI refresh.

In source, active area regions live in `ScrArea.regionbase`. Blender resolves
each region's runtime type from the active area `SpaceType`, and region drawing
calls the region's draw callback. This lets every editor have different chrome
while still participating in the same area/region lifecycle.

Implementation effect:

- Headers, sidebars, toolbars, and main content are not separate global layout
  primitives.
- Each editor can define its own internal structure while area management stays
  uniform.
- Redraw and refresh can target a region instead of the whole application.

Sources:

- Blender Regions manual.
- Blender Python API `bpy.types.Region`.
- `source/blender/editors/screen/area.cc`: `ED_area_and_region_types_init`,
  `ED_region_do_draw`.
- <https://docs.blender.org/manual/en/latest/interface/window_system/regions.html>
- <https://docs.blender.org/api/current/bpy.types.Region.html>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L2222-L2236>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L488-L530>

### 6. Split, join, swap, maximize, and duplicate are first-class area actions

The Areas manual documents concrete area operations: dragging an area corner to
split, dragging into another area to join or replace, using area options for
split/join/swap, swapping contents with a corner drag, duplicating an area into
a new window, maximizing an area, and toggling fullscreen.

In source, areas have `actionzones`. `area_azone_init()` rebuilds those handles
for normal screens and skips global, temporary, locked, or non-normal cases.
`ED_area_swapspace()` exits both areas, copies their data through a temporary
area, reinitializes both, clears stale active-region pointers, adds a mousemove,
and tags both areas for redraw and refresh.

Implementation effect:

- The user manipulates visible corners, borders, and areas rather than hidden
  layout commands only.
- Area operations operate on area data and then reinitialize editor runtime.
- Swapping areas moves editor contents while preserving the layout frame.

Sources:

- Blender Areas manual.
- Blender Python API screen operators: `area_split`, `area_join`,
  `area_swap`, `area_dupli`, `screen_full_area`.
- `source/blender/editors/screen/area.cc`: action zones, area data swap,
  `ED_area_swapspace`.
- <https://docs.blender.org/api/current/bpy.ops.screen.html>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L1047-L1062>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L2480-L2496>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L2741-L2775>

### 7. Editor switching preserves previous space state

`ED_area_newspace()` is the most instructive path for Wardian. When a user
changes an area's editor type, Blender:

1. Exits the current area.
2. Sets `area->spacetype` and `area->type`.
3. Searches `area->spacedata` for an existing `SpaceLink` of the requested
   type.
4. If found, swaps the active `regionbase` with that space's stored regions and
   moves the space to the front of the list.
5. If not found, calls the target `SpaceType` create callback, adds the new
   space to the front, and makes its regions active.

Implementation effect:

- An area can be retargeted without becoming a blank editor every time.
- Per-editor state follows the editor instance, while the rectangular area
  remains stable.
- The "slot" and the "thing hosted by the slot" are separate concepts.

Sources:

- Blender Python API `Area.spaces`.
- `source/blender/editors/screen/area.cc`: `area_init_type_fallback`,
  `ED_area_newspace`.
- <https://docs.blender.org/api/current/bpy.types.Area.html>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L2192-L2219>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L2777-L2870>

### 8. Workspaces remember layout per window

`WorkSpaceDataRelation` stores per-workspace relations to non-workspace data.
The source comment explains the user-facing behavior: when activating a
workspace, Blender should activate the screen layout that was active before in
this window. If two windows use the same workspace history differently, each
window gets its own remembered active screen layout.

`WorkSpaceInstanceHook` then stores the active workspace and active layout for
a window. Window manager helpers read and write active workspace, layout, and
screen through that hook.

Implementation effect:

- Workspace switching feels stable in multi-window setups.
- Layout memory is scoped to the workspace/window relation, not a single global
  active layout.
- Temporary screens can be excluded from broad workspace changes.

Sources:

- `source/blender/makesdna/DNA_workspace_types.h`: `WorkSpaceDataRelation`,
  `WorkSpaceInstanceHook`.
- `source/blender/windowmanager/intern/wm_window.cc`: active workspace/layout
  helpers.
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/makesdna/DNA_workspace_types.h#L200-L252>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/windowmanager/intern/wm_window.cc#L3369-L3414>

### 9. Tools are workspace, editor, and mode aware

The workspace stores tools, but tool selection is resolved per screen area.
`WM_toolsystem_refresh_screen_area()` computes the mode from the current space
type and active scene/view layer/object context, then selects the matching
workspace tool whose `space_type` and `mode` match the area.

Implementation effect:

- The same workspace can contain multiple editor areas with different tool
  affordances.
- Tool state follows the task context instead of being a single global toolbar.
- The 3D scene/object model remains the center of gravity, even while the UI is
  modular.

Sources:

- `source/blender/windowmanager/intern/wm_toolsystem.cc`:
  `WM_toolsystem_mode_from_spacetype`,
  `WM_toolsystem_refresh_screen_area`.
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/windowmanager/intern/wm_toolsystem.cc#L740-L750>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/windowmanager/intern/wm_toolsystem.cc#L867-L907>

### 10. Attention and redraw are scoped to areas and regions

Area and region listeners, refresh callbacks, and draw tags are explicit.
`ED_area_do_listen()` delegates notifier handling to the active area
`SpaceType`. `ED_region_do_draw()` calls the active region type's draw callback.
`ED_region_tag_redraw()`, `ED_region_tag_redraw_partial()`, and
`ED_area_tag_redraw()` allow Blender to mark a whole area or only a region/rect
for redraw.

Implementation effect:

- Smoothness comes partly from invalidating the right surface, not repainting
  everything.
- Editor-specific listeners stay inside editor types.
- Region-level attention maps well to headers, toolbars, sidebars, and main
  canvases.

Sources:

- `source/blender/editors/screen/area.cc`: listeners, region drawing, redraw
  tagging.
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L119-L145>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/screen/area.cc#L626-L708>

### 11. Drag and drop is region/context specific

Blender's drag/drop system does not imply every object is a universal draggable
view. Drop handling is mediated through registered dropboxes. During drag
invoke, Blender builds a matrix of visible area space types and region types,
then uses that to decide which dropbox maps are relevant.

Implementation effect:

- Drag/drop legality is contextual: a dragged thing is accepted by compatible
  visible editor regions.
- Editors can own their own drop behavior through registered space/dropbox
  callbacks.
- This keeps drag affordances powerful without making every part of the UI
  accept every kind of payload.

Sources:

- `source/blender/windowmanager/intern/wm_dragdrop.cc`: visible
  space/region matrix for dropbox prefetch.
- `source/blender/editors/space_api/spacetypes.cc`: space type dropbox
  registration.
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/windowmanager/intern/wm_dragdrop.cc#L257-L285>
- <https://github.com/blender/blender/blob/bd0823aa964fd8f6aff8c8ce2ef5a88d5df47cbd/source/blender/editors/space_api/spacetypes.cc#L177-L183>

## Why Blender Feels Smooth

Blender's smoothness comes from strong constraints:

- The screen is a subdivided area map.
- The area is the manipulable surface.
- The editor type is registered behind a common contract.
- Regions handle the editor's internal chrome.
- Editor switching preserves previous local state.
- Workspace layout memory is persistent and scoped to the active window.
- Tools and shortcuts are context-aware rather than globally flat.
- Redraw and notification are scoped to the area/region that changed.

The result is a UI that feels modular without becoming arbitrary. Blender does
not ask the user to assemble an application from raw components. It gives the
user a stable physical workspace made of durable editor slots.

## Wardian Implications

Blender suggests a stronger Wardian model than generic draggable cards:

```text
HabitatWorkspace
  -> HabitatLayout
    -> SurfaceMap
      -> SurfaceArea
        -> SurfaceState
        -> SurfaceType
        -> SurfaceRegion
```

Practical design implications:

- Treat the agent/task/session as the shared context center, analogous to
  Blender's scene/object/mode context.
- Make the manipulable primitive a durable surface area, not a raw React
  component.
- Let a surface switch type: terminal, browser, file, transcript, diff,
  planner, metrics, runbook, skill library, agent roster, or workflow graph.
- Preserve previous per-type state inside each surface, so switching a surface
  away from terminal and back restores scrollback, session binding, and local
  UI state.
- Give every surface type a registry contract: create, init, dispose, render,
  commands, drag/drop payloads, status, keybindings, and region definitions.
- Split surface chrome into regions: header, status strip, command rail,
  inspector/sidebar, main content, and overlays.
- Use visible edge/corner action zones for split, join, swap, duplicate,
  maximize, and detach actions.
- Normalize drag payloads by type and target region before mutating layout.
- Scope attention/status/redraw to surface regions, so a terminal update,
  agent status change, browser navigation, and file diff change do not all
  repaint or visually compete at the same level.

## Design Principle

Blender centers on task workspaces made of durable editor areas. Its
malleability comes from disciplined surface slots and registered editor types,
not from unconstrained component freedom. For Wardian, that points toward a
physical Habitat grid where agent-adjacent surfaces can be split, swapped,
retargeted, and restored while keeping the active agent/task context coherent.
