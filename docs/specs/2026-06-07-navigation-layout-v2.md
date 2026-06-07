# Navigation and Layout V2

- **Status:** Exploratory
- **Date:** 2026-06-07

## Context and Problem Statement

Wardian's current desktop UI is a dense command-center shell:

```text
App shell
  -> titlebar view modes
  -> left activity rail and left contextual pane
  -> central main view
  -> right agent watchlist
  -> optional bottom user terminal
  -> modal settings
```

The current titlebar view modes are implemented as full-screen main views:
`grid`, `dashboard`, `queue`, `graph`, `garden`, `library`, and `workflows`.
The left activity rail hosts contextual tools: explorer, source control, agent
configuration/spawn, command broadcast, class manager, and workflow glance. The
right side hosts the agent watchlist. The bottom area can host the user
terminal.

Research in `docs/research/malleable-gui-references/` suggests a better long
term model. cmux is strongest for Wardian because it treats terminals, browser,
files, markdown, and agent-oriented panels as peer surfaces in one split/tab
workspace. Obsidian is the strongest reference for the layout boundary: a
durable leaf moves through the workspace tree, while the view inside the leaf
owns content state but not layout mechanics.

The risk is flattening Wardian's current product structure into vague
"surfaces." Current main views, auxiliary panels, and agent session
presentations are not the same kind of object. Navigation/layout v2 should
preserve those distinctions.

This spec is intentionally a starting point. It records the corrected taxonomy
before implementation planning.

## Proposed Decision

Navigation/layout v2 should model the application through six related
concepts:

```text
PerspectiveDefinition
  -> creates or updates a Workspace

Workspace
  -> owns ShellRegions and a MainWorkspace

MainWorkspace
  -> owns SurfaceLeaves

SurfaceLeaf
  -> hosts one SurfaceView

SurfaceView
  -> owns SurfaceModes and SurfaceRegions
```

### 1. Shell Regions

Shell regions are stable placement zones. They are not surface types.

```text
ShellRegion:
  - titlebar
  - left-rail
  - left-dock
  - main-workspace
  - right-dock
  - bottom-dock
  - modal-layer
```

The left dock and right dock should be treated symmetrically at the architecture
level. Today they differ by product role, not by layout type:

- Left dock: contextual tools selected by the activity rail.
- Right dock: persistent agent watchlist/roster.
- Bottom dock: user terminal today; later also logs, queue, approvals, or run
  output.

### 2. Perspectives

Perspectives replace the current titlebar `viewMode` concept, but they should
not become a larger set of mutually exclusive global pages. A perspective is a
definition or template: a saved, predefined, plugin-contributed, or user-created
arrangement of shell regions, auxiliary panels, and main workspace surfaces.

The live object is the workspace. A workspace may be created from a perspective
definition, may keep a base perspective identity for orientation, and may then
diverge as the user opens surfaces, splits leaves, promotes panels, and pins
tools.

```text
PerspectiveDefinition
  -> describes the kind of work and default layout

Workspace
  -> live/saved user context created from or associated with a perspective

SurfaceTree
  -> actual split/tab graph inside the workspace
```

Current main views map first to perspective definitions, not directly to
primitive surface types:

```text
PerspectiveDefinition:
  - command-center      // current grid/dashboard lineage
  - agent-graph         // current graph view
  - queue-inbox         // current queue view
  - workflows           // current workflows view
  - library             // current library view
  - garden              // future habitat mode
  - plugin-defined      // plugin-contributed templates
```

The titlebar should eventually switch the current workspace context and expose a
small pinned set of perspectives or workspaces. It should not list every main
view, every plugin, or every open work object.

### 3. Perspective Navigation Models

Wardian should support several ways to reach a perspective instead of choosing
one global navigation primitive.

```text
Home / start surface
  -> launch perspective definitions, recent workspaces, saved layouts, and
     plugin templates

Titlebar
  -> show current workspace, a small pinned set, overflow, and quick switching

Command palette / quick open
  -> open or focus any perspective, workspace, surface, panel, or command

Surface tabs and splits
  -> manage the open work objects inside the active workspace
```

The current global topbar tabs are useful for a small fixed product, but they
scale poorly once plugins can contribute surfaces and templates. A Home view is
useful as a launcher, especially for empty states and saved layouts, but it
should not become a mandatory hub that users must pass through during normal
work.

Recommended model:

```text
Home answers:       what kind of work do I want to start?
Titlebar answers:   which workspace/context am I in?
Surface tabs answer: which work objects are open here?
Quick open answers: where can I jump immediately?
```

This makes plugins more malleable without giving every plugin a permanent
global tab. Plugins can contribute surface types, auxiliary panel types,
commands, perspective definitions, and suggested default placements. Users can
pin plugin perspectives, plugin surfaces, or saved plugin-heavy workspaces into
the titlebar or Home.

### 4. Auxiliary Panels

Auxiliary panels are docked tools. They live in left, right, or bottom regions
by default. They may be promoted into the main workspace later, but they are not
primary main surfaces by default.

```text
AuxiliaryPanelType:
  - file-explorer
  - source-control
  - agent-config
  - spawn-agent
  - command-broadcast
  - class-manager
  - workflow-glance
  - agent-watchlist
  - queue-glance
  - terminal-drawer
```

Current mapping:

- `ExplorerPanel` -> `file-explorer`
- `GitPanel` -> `source-control`
- `ConfigureAgentPanel` and `SpawnAgentPanel` -> `agent-config` /
  `spawn-agent`
- `CommandPanel` -> `command-broadcast`
- `ClassManagerPanel` -> `class-manager`
- `WorkflowsGlancePane` -> `workflow-glance`
- `AgentWatchlist` -> `agent-watchlist`
- `UserTerminalPanel` -> `terminal-drawer` by default

The important correction is that the right agent watchlist is not a special
main surface. It is an auxiliary panel, structurally comparable to the left
side tools. It is more persistent because it is important for orientation.

### 5. Main Workspace Surfaces

Main workspace surfaces are the cmux/Obsidian-style work objects: movable,
tabbable, splittable, restorable leaves inside the main workspace.

Core v2 surface candidates:

```text
SurfaceType:
  - agent-session
  - file-viewer
  - browser
  - workflow-workbench
  - user-terminal
```

Secondary or later surface candidates:

```text
SurfaceType:
  - agent-graph
  - habitat-canvas
  - library-browser
  - queue-inbox
  - plugin-surface
  - promoted-panel
```

Promotion is explicit. A left or right auxiliary panel can become a main
surface when the user needs more space or wants it beside another work object.
For example:

- Source control can be promoted beside a file viewer.
- Agent watchlist can be promoted for triage or team comparison.
- Queue can be promoted during action-needed review.
- Workflow glance can be promoted into the full workflow workbench.

Garden and Queue side by side should initially be represented as two surfaces
in the same `SurfaceTree`, for example `habitat-canvas` beside `queue-inbox`.
It should not require nested perspectives in the first implementation. A future
`PerspectiveInstance` container could support that, but it should be deferred
until the simpler surface-tree model proves insufficient.

Plugin surfaces should behave like local work objects. They can be opened,
tabbed, split, saved, restored, and promoted according to their declared
capabilities. They do not receive permanent global titlebar placement by
default.

### 6. Surface Modes and Regions

Several current or proposed "surfaces" are better understood as modes or
regions inside a larger surface.

An agent terminal and agent chat should not be separate default leaves. The
user cares about the agent session. Terminal, chat, transcript, metrics, and
tool-call views are presentations of the same agent session.

```text
AgentSessionSurface:
  context:
    - agent_session_id
    - workspace_path
    - provider
    - status
  modes:
    - terminal
    - chat
    - transcript
    - terminal-chat-split
  regions:
    - header
    - main
    - composer
    - status/metrics
```

Likewise, the workflow view is a workbench surface with internal modes:

```text
WorkflowWorkbenchSurface:
  modes:
    - builder
    - monitor
    - observe-run
    - schedule-editor
```

The idealized file capability is `file-viewer`, not `file-preview`.

```text
FileViewerSurface:
  context:
    - workspace_path
    - file_path
  modes:
    - read
    - edit
    - diff
    - blame
  state:
    - cursor
    - selection
    - scroll
    - dirty/read-only
```

## Current-to-V2 Mapping

```text
GridView
  -> command-center perspective containing agent-session surfaces

DashboardView
  -> command-center overview mode, or later telemetry/agent-summary surface

GraphView
  -> agent-graph surface inside agent-graph perspective

QueueView
  -> queue-inbox surface or queue-inbox perspective

WorkflowsView
  -> workflows perspective containing workflow-workbench surface

LibraryView
  -> library perspective containing library-browser surface

ExplorerPanel
  -> file-explorer auxiliary panel; opens file-viewer surfaces

GitPanel
  -> source-control auxiliary panel; opens file-viewer or diff modes

AgentWatchlist
  -> agent-watchlist auxiliary panel; opens or focuses agent-session surfaces

UserTerminalPanel
  -> terminal-drawer auxiliary panel by default; can later become user-terminal surface
```

## Early Data Model Sketch

The exact schema will need a follow-up implementation spec. The shape should
separate perspective definitions, live workspaces, docked panels, and main
workspace leaves.

```ts
type LayoutV2State = {
  version: 1;
  active_workspace_id: string;
  workspaces: WorkspaceState[];
  perspective_definitions: PerspectiveDefinition[];
  pinned_entries: PinnedNavigationEntry[];
};

type PerspectiveDefinition = {
  id: string;
  name: string;
  source: "core" | "plugin" | "user";
  description?: string;
  template: PerspectiveTemplate;
};

type PerspectiveTemplate = {
  shell?: Partial<ShellRegionState>;
  main_workspace: SurfaceTreeTemplate;
  recommended_pins?: PinnedNavigationEntry[];
};

type SurfaceTreeTemplate = SurfaceTree;

type WorkspaceState = {
  id: string;
  name: string;
  base_perspective_id?: string;
  shell: ShellRegionState;
  main_workspace: SurfaceTree;
};

type PinnedNavigationEntry =
  | { type: "workspace"; workspace_id: string }
  | { type: "perspective"; perspective_definition_id: string }
  | { type: "surface"; surface_view_id: string };

type ShellRegionState = {
  left_dock: DockState;
  right_dock: DockState;
  bottom_dock: DockState;
};

type DockState = {
  collapsed: boolean;
  active_panel_id?: string;
  panels: AuxiliaryPanelState[];
  size_px?: number;
};

type SurfaceTree =
  | { type: "leaf"; leaf: SurfaceLeafState }
  | { type: "split"; direction: "row" | "column"; children: SurfaceTree[]; sizes: number[] }
  | { type: "tabs"; active_leaf_id: string; leaves: SurfaceLeafState[] };

type SurfaceLeafState = {
  id: string;
  view: SurfaceViewState;
};

type SurfaceViewState =
  | AgentSessionSurfaceState
  | FileViewerSurfaceState
  | BrowserSurfaceState
  | WorkflowWorkbenchSurfaceState
  | UserTerminalSurfaceState
  | AgentGraphSurfaceState
  | HabitatCanvasSurfaceState
  | LibraryBrowserSurfaceState
  | QueueInboxSurfaceState
  | PluginSurfaceState
  | PromotedPanelSurfaceState;
```

## Design Rules

- Perspectives define work arrangements; workspaces hold live work.
- Perspectives are templates and starting points, not necessarily mutually
  exclusive full-screen pages.
- Auxiliary panels control, navigate, and inspect.
- Surfaces are movable work objects.
- Surface leaves own placement; surface views own content state.
- Plugin surfaces get local surface tabs and splits by default. Global titlebar
  placement is user-pinned or core-curated, not automatic.
- Home launches perspective definitions, recent workspaces, saved layouts, and
  plugin templates. It should not be a mandatory route between normal tasks.
- Multiple "perspectives" side by side should initially mean surfaces from
  different perspective definitions living in one surface tree.
- Agent terminal/chat/transcript are modes or regions of an `agent-session`
  surface.
- `file-viewer` is a real first-class work surface, not a lightweight preview.
- Left and right docks are architecturally symmetric, even if the right roster
  remains persistent by default.
- Do not make every component draggable. Make durable leaves draggable.

## Consequences

- **Positive:** Aligns Wardian with the strongest cmux pattern: multiple tool
  surfaces sharing one split/tab workspace.
- **Positive:** Preserves the strongest Obsidian pattern: layout leaves move
  independently from view content.
- **Positive:** Avoids treating current main views, side panels, and
  presentation modes as the same object.
- **Positive:** Keeps the current dense command-center shell recognizable while
  creating a path toward more malleable workspaces.
- **Positive:** Allows Garden and Queue, or a plugin workbench and an agent
  session, to sit side by side without inventing nested global modes.
- **Positive:** Gives plugins meaningful layout participation without letting
  plugin count overwhelm the global titlebar.
- **Negative:** Requires a model migration from `viewMode` and the existing
  `useLayoutStore` shape.
- **Negative:** Some current views may need to split into perspective,
  auxiliary-panel, and surface implementations.
- **Negative:** A real `file-viewer` and browser surface require stronger
  lifecycle, focus, and persistence handling than placeholder panels.
- **Negative:** Introduces more vocabulary. Product copy and developer APIs
  must make the difference between perspective, workspace, panel, surface, and
  mode obvious.
- **Negative:** Plugin contribution governance becomes part of the layout
  system; otherwise plugins can degrade navigation quality.

## Additional Considerations

### Persistence and Migration

Layout state should be versioned from the start. The first implementation can
keep most state in the frontend store, but the schema should be backend-readable
eventually so agents, CLI commands, and Markdown-backed project state can
inspect open workspaces and surface handles. Migration should map the existing
`viewMode` values into core `PerspectiveDefinition` entries and likely turn the
current topbar choices into default pinned entries.

### Plugin Boundaries

Plugins should declare what they contribute:

```text
PluginContribution:
  - surface type
  - auxiliary panel type
  - command
  - perspective definition
  - suggested default placement
```

Core Wardian should validate contribution IDs, icons, labels, default placement,
and persistence payloads. A plugin should not be able to silently reserve global
navigation, override core perspectives, or store opaque layout state that cannot
be migrated.

### Command and Addressing Model

Every open work object needs a stable handle. CLI commands, agents, command
palette actions, and future automation should be able to address:

```text
workspace_id
surface_leaf_id
surface_view_id
auxiliary_panel_id
perspective_definition_id
```

This is especially important for multi-agent workflows where an agent might
open a file viewer, focus a running terminal, request a browser surface, or
place a queue inbox beside the current session.

### Surface Lifecycle and Fidelity

Terminals, browser surfaces, file viewers, and agent sessions have different
mount/unmount costs. The layout system needs explicit lifecycle policies for
hidden tabs, background splits, suspended surfaces, restored PTYs, dirty files,
and provider sessions. Smooth malleability will fail if moving a leaf destroys
terminal scrollback, browser state, or file cursor state.

### Accessibility and Keyboard Use

The v2 model needs keyboard-first navigation from the beginning: quick switcher,
surface focus traversal, split movement, tab movement, dock toggles, and
promotion/demotion commands. Surface tabs and dock selectors should map to real
ARIA tab/menu patterns rather than custom click-only controls.

### Responsive Layout

Small screens should emphasize perspective/workspace picking and one active
surface at a time. Splits can collapse into tab stacks, side docks can become
drawers, and Home can become more important as a launcher. The data model
should survive these responsive transforms without creating a separate mobile
layout concept.

### Testing and Rollout

The first implementation plan should include schema snapshot tests, migration
tests from the existing layout store, browser E2E coverage for opening,
splitting, tabbing, promoting, and restoring surfaces, and screenshot evidence
for the changed navigation states. Native tests become necessary once terminal
or provider session lifecycle claims are part of the change.

## Open Questions

- Which first slice proves the model best: command-center workspace, workflows
  workbench, or agent-graph plus queue side-by-side?
- What should Home contain in v2: pinned perspectives only, recent workspaces,
  active agent sessions, saved layouts, plugin templates, or all of these?
- Which titlebar entries should be pinned by default after migrating from the
  current global view tabs?
- Should `user-terminal` remain bottom-dock-only for v2, or become a main
  surface immediately?
- Which auxiliary panels should be promotable in the first slice?
- Which plugin contribution types should be accepted first?
- Are nested perspective instances worth supporting later, or should Wardian
  keep all side-by-side composition at the surface-tree layer?
- Should layout v2 persist only in frontend state for the first implementation,
  or should it be backend-readable from the start?
