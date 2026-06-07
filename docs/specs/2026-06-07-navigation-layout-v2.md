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

Navigation/layout v2 should model the application in five layers:

```text
ShellRegion
  -> Perspective
    -> MainWorkspace
      -> SurfaceLeaf
        -> SurfaceView
          -> SurfaceMode / SurfaceRegion
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

Perspectives replace the current titlebar `viewMode` concept. A perspective is
a saved or predefined arrangement of shell regions, auxiliary panels, and main
workspace surfaces.

Current main views map first to perspectives, not directly to primitive surface
types:

```text
Perspective:
  - command-center      // current grid/dashboard lineage
  - agent-graph         // current graph view
  - queue-inbox         // current queue view
  - workflows           // current workflows view
  - library             // current library view
  - garden              // future habitat mode
```

The titlebar should eventually switch perspectives. It should not be the only
way to open a work object.

### 3. Auxiliary Panels

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

### 4. Main Workspace Surfaces

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
  - library-browser
  - queue-inbox
  - promoted-panel
```

Promotion is explicit. A left or right auxiliary panel can become a main
surface when the user needs more space or wants it beside another work object.
For example:

- Source control can be promoted beside a file viewer.
- Agent watchlist can be promoted for triage or team comparison.
- Queue can be promoted during action-needed review.
- Workflow glance can be promoted into the full workflow workbench.

### 5. Surface Modes and Regions

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
separate perspective, docked panels, and main workspace leaves.

```ts
type LayoutV2State = {
  version: 1;
  active_perspective_id: string;
  perspectives: PerspectiveState[];
};

type PerspectiveState = {
  id: string;
  name: string;
  shell: ShellRegionState;
  main_workspace: SurfaceTree;
};

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
  | LibraryBrowserSurfaceState
  | QueueInboxSurfaceState
  | PromotedPanelSurfaceState;
```

## Design Rules

- Perspectives arrange work.
- Auxiliary panels control, navigate, and inspect.
- Surfaces are movable work objects.
- Surface leaves own placement; surface views own content state.
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
- **Negative:** Requires a model migration from `viewMode` and the existing
  `useLayoutStore` shape.
- **Negative:** Some current views may need to split into perspective,
  auxiliary-panel, and surface implementations.
- **Negative:** A real `file-viewer` and browser surface require stronger
  lifecycle, focus, and persistence handling than placeholder panels.

## Open Questions

- Which perspective should be the first implementation target:
  command-center, workflows, or agent-graph?
- Should `user-terminal` remain bottom-dock-only for v2, or become a main
  surface immediately?
- Which auxiliary panels should be promotable in the first slice?
- Should layout v2 persist only in frontend state at first, or should it be
  backend-readable from the start?
- How should CLI/agent commands address surface handles once v2 exists?
