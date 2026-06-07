# Navigation and Layout V2

- **Status:** Exploratory
- **Date:** 2026-06-07

## Context and Problem Statement

Wardian's current desktop UI is a dense multi-agent shell:

```text
App shell
  -> titlebar view modes
  -> left activity rail and left contextual pane
  -> central main view
  -> right agent roster
  -> optional bottom user terminal
  -> modal settings
```

The current titlebar view modes are implemented as full-screen main views:
`grid`, `dashboard`, `queue`, `graph`, `garden`, `library`, and `workflows`.
The left activity rail hosts contextual tools: explorer, source control, agent
configuration/spawn, command broadcast, class manager, and workflow glance. The
right side hosts the agent roster. The bottom area can host the user terminal.

Research in `docs/research/malleable-gui-references/` suggests a better long
term model. cmux is strongest for Wardian because it treats terminals, browser,
files, markdown, and agent-oriented panels as peer surfaces in one split/tab
surface area. Obsidian is the strongest reference for the layout boundary: a
durable leaf moves through the layout tree, while the view inside the leaf owns
content state but not layout mechanics.

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
  -> creates or updates a LayoutContext

LayoutContext
  -> owns ShellRegions and a SurfaceCanvas

SurfaceCanvas
  -> owns SurfaceLeaves

SurfaceLeaf
  -> hosts one SurfaceView

SurfaceView
  -> owns SurfaceModes and SurfaceRegions
```

Internal model names and product labels do not need to be identical. The
recommended user-facing term for `LayoutContext` is **Site**:

```text
LayoutContext     // internal/API term
Site              // product label
```

A Site is a saved operating place inside Wardian's Habitat: active perspective
lineage, surface canvas, dock state, and active Cohort. This keeps the
ecological theme at the product level while avoiding the overloaded `workspace`
term, which already means the project path or worktree used by an
agent/provider.

The broader hierarchy should be:

```text
Habitat
  -> whole Wardian environment

Garden
  -> spatial overview of the Habitat

Site
  -> saved/focused operating place inside the Habitat

Cohort
  -> group of agents associated with or currently occupying a Site

Surface
  -> movable work object inside a Site
```

This makes Garden cleaner: an all-agents Garden can show multiple Sites, each
with Cohorts arranged around or inside them. A Cohort-focused Garden can open a
single Site for focused work.

Alternatives considered:

- `Workspace`: rejected because it conflicts with agent workspaces.
- `Habitat`: too broad; it should remain the larger Wardian environment.
- `Station`: too command-center oriented for Wardian's ecological theme.
- `Desk`: familiar, but too generic and less ecological.
- `Lens`: useful metaphor for filtering, but too narrow for a full saved
  layout.
- `Deck`: tactile, but can imply cards/trading and is less obvious than
  Site.

### 1. Shell Regions

Shell regions are stable placement zones. They are not surface types.

```text
ShellRegion:
  - titlebar
  - left-rail
  - left-dock
  - main-surface-area
  - right-dock
  - bottom-dock
  - modal-layer
```

The left dock and right dock should be treated symmetrically at the architecture
level. Today they differ by product role, not by layout type:

- Left dock: contextual tools selected by the activity rail.
- Right dock: persistent agent roster.
- Bottom dock: user terminal today; later also logs, queue, approvals, or run
  output.

### 2. Perspectives

Perspectives replace the current titlebar `viewMode` concept, but they should
not become a larger set of mutually exclusive global pages. A perspective is a
definition or template: a saved, predefined, plugin-contributed, or user-created
arrangement of shell regions, auxiliary panels, and central surfaces.

The live object is the layout context. A layout context may be created from a
perspective definition, may keep a base perspective identity for orientation,
and may then diverge as the user opens surfaces, splits leaves, promotes panels,
switches Cohorts, and pins tools.

Do not call this object a workspace. In Wardian, an agent workspace already
means the target project directory or worktree where the provider runs.
Overloading that term would blur the difference between "where an agent is
working on disk" and "how the user has arranged the command-center UI."

```text
PerspectiveDefinition
  -> describes the kind of work and default layout

LayoutContext
  -> live/saved UI context created from or associated with a perspective

SurfaceTree
  -> actual split/tab graph inside the layout context
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

The titlebar should eventually show the current Site and expose a small pinned
set of perspectives or saved Sites. It should not list
every main view, every plugin, or every open work object.

### 3. Perspective Navigation Models

Wardian should support several ways to reach a perspective instead of choosing
one global navigation primitive.

```text
Home / start surface
  -> launch perspective definitions, recent Sites, saved layouts, and
     plugin templates

Titlebar
  -> show current Site, a small pinned set, overflow, and quick switching

Command palette / quick open
  -> open or focus any perspective, Site, surface, panel, or command

Surface tabs and splits
  -> manage the open work objects inside the active Site
```

The current global topbar tabs are useful for a small fixed product, but they
scale poorly once plugins can contribute surfaces and templates. A Home view is
useful as a launcher, especially for empty states and saved layouts, but it
should not become a mandatory hub that users must pass through during normal
work.

Recommended model:

```text
Home answers:       what kind of work do I want to start?
Titlebar answers:   which Site am I in?
Surface tabs answer: which work objects are open here?
Quick open answers: where can I jump immediately?
```

This makes plugins more malleable without giving every plugin a permanent
global tab. Plugins can contribute surface types, auxiliary panel types,
commands, perspective definitions, and suggested default placements. Users can
pin plugin perspectives, plugin surfaces, or saved plugin-heavy Sites into
the titlebar or Home.

### 4. Cohorts as Working Sets

Current watchlists and teams are already Wardian's closest analogue to the
"workspace" concept in traditional IDEs. The v2 user-facing name should move
away from **Watchlist**, which came from earlier TradingView-inspired drafting.
The recommended replacement is **Cohort**.

Cohorts define which agents are visible, targetable, grouped, and operationally
relevant right now. That role should be preserved instead of replaced by a new
IDE-style workspace layer.

A Cohort should not be permanently owned by one Site. A Site can have an active
Cohort, and a Cohort can be associated with multiple Sites over time. This
keeps "Review Lane" or "Frontend Ops" reusable across different project
moments instead of making the grouping inseparable from one saved layout.

```text
AgentWorkspace
  -> filesystem path or worktree used by an agent/provider

Cohort / Team
  -> durable agent working set and targeting scope

LayoutContext
  -> saved UI arrangement, active perspective lineage, open surfaces, docks,
     and active Cohort/filter state
```

The right roster sidebar is therefore more than a passive list. It is the active
working-set selector for many Wardian tasks. Layout v2 should let a Site
remember the active Cohort, visible teams, and collapsed team state. It
should not persist selected agents, focused surface, or generic scroll state in
the first slice; selected agents and focus are intertwined interaction state,
while scroll restoration is extra complexity unless tied to a specific surface
such as a terminal or file viewer.

A Site for "Frontend Ops" can share the same perspective definition as "Review
Lane" while using a different Cohort and different open surfaces.

Switching Cohorts should be seamless. It should filter what is shown and
targetable, not turn agents on or off, pause sessions, hide provider state from
the backend, or destroy terminal surfaces. For v2, switching between
substantially different working sets should usually switch the whole Site:
the active Cohort, open surfaces, docks, and perspective lineage move
together. This makes "Frontend Ops" and "Review Lane" feel like separate
operating setups rather than one layout with a temporary filter applied.

This also clarifies a limitation of Cohorts: they organize agents, not the
entire UI. They do not by themselves remember splits, open files, browser state,
queue surfaces, or dock configuration. Sites fill that gap without stealing
the name or role of agent workspaces.

Garden should use the same distinction. The all-agents Garden is the Habitat
overview: multiple Sites visible, with Cohorts arranged around or inside each
Site. A Cohort-focused Garden opens the one active Site for that Cohort and
shows the local surfaces, queue, files, terminals, and state for focused work.

### 5. Site Persistence Boundary

A Site should remember the parts of the operating setup that make switching
between working sets feel seamless:

```text
Site remembers:
  - base perspective definition
  - surface canvas tree
  - open surface views and their surface-owned state
  - shell region and dock sizes/collapsed state
  - promoted auxiliary panels
  - active Cohort
  - visible teams and collapsed team state
  - user pins relevant to the Site
```

A Site should not remember these in the first slice:

```text
Site does not remember:
  - selected agents
  - focused surface
  - generic page scroll state
  - agent lifecycle state
  - provider session ownership
  - agent/project workspace identity
```

Selected agents and focused surface are tightly coupled interaction state; they
can be recomputed from the active Cohort and currently visible surfaces.
Scroll state should be owned only by specific surfaces that need it, such as
file viewers, browser surfaces, and terminal surfaces. Agent lifecycle and
workspace identity remain backend/provider concerns, not layout concerns.

### 6. Auxiliary Panels

Auxiliary panels are docked tools. They live in left, right, or bottom regions
by default. They may be promoted into the main surface area later, but they are
not primary main surfaces by default.

```text
AuxiliaryPanelType:
  - file-explorer
  - source-control
  - agent-config
  - spawn-agent
  - command-broadcast
  - class-manager
  - workflow-glance
  - agent-roster
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
- `AgentWatchlist` -> `agent-roster`
- `UserTerminalPanel` -> `terminal-drawer` by default

The important correction is that the right agent roster is not a special main
surface. It is an auxiliary panel, structurally comparable to the left side
tools. It is more persistent because it is important for orientation.

### 7. Main Surface Area

Surfaces in the main surface area are the cmux/Obsidian-style work objects:
movable, tabbable, splittable, restorable leaves inside the central surface
canvas.

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
- Agent roster can be promoted for triage or team comparison.
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

### 8. Surface Modes and Regions

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

GardenView
  -> garden perspective containing habitat-canvas surface; all-agents mode
     shows multiple Sites, Cohort mode opens one Site

WorkflowsView
  -> workflows perspective containing workflow-workbench surface

LibraryView
  -> library perspective containing library-browser surface

ExplorerPanel
  -> file-explorer auxiliary panel; opens file-viewer surfaces

GitPanel
  -> source-control auxiliary panel; opens file-viewer or diff modes

AgentWatchlist
  -> agent-roster auxiliary panel; opens or focuses agent-session surfaces

UserTerminalPanel
  -> terminal-drawer auxiliary panel by default; can later become user-terminal surface
```

## Early Data Model Sketch

The exact schema will need a follow-up implementation spec. The shape should
separate perspective definitions, live layout contexts, docked panels, and
central surface leaves.

```ts
type LayoutV2State = {
  version: 1;
  active_layout_context_id: string;
  layout_contexts: LayoutContextState[];
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
  surface_canvas: SurfaceTreeTemplate;
  recommended_pins?: PinnedNavigationEntry[];
};

type SurfaceTreeTemplate = SurfaceTree;

type LayoutContextState = {
  id: string;
  name: string;
  label: "Site";
  base_perspective_id?: string;
  active_cohort_id?: string;
  collapsed_team_ids?: string[];
  shell: ShellRegionState;
  surface_canvas: SurfaceTree;
};

type PinnedNavigationEntry =
  | { type: "layout-context"; layout_context_id: string }
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

- Perspectives define work arrangements; layout contexts hold live UI state.
- The user-facing label for a layout context should be Site unless product
  testing finds a clearer term.
- Perspectives are templates and starting points, not necessarily mutually
  exclusive full-screen pages.
- Auxiliary panels control, navigate, and inspect.
- Surfaces are movable work objects.
- Surface leaves own placement; surface views own content state.
- Plugin surfaces get local surface tabs and splits by default. Global titlebar
  placement is user-pinned or core-curated, not automatic.
- Home launches perspective definitions, recent Sites, saved layouts, and
  plugin templates. It should not be a mandatory route between normal tasks.
- Sites remember active Cohort and collapsed team state. They should
  not persist selected agents, focused surface, or generic scroll state in the
  first slice.
- Cohort switching filters visible/targetable agents. It must not turn
  agents on/off or mutate provider lifecycle state.
- Switching between materially different working sets should usually switch the
  whole Site, not only apply a temporary cohort filter.
- Garden can render an all-agents Habitat view as multiple Sites arranged with
  their Cohorts, while a Cohort-focused Garden opens one Site.
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
  surfaces sharing one split/tab surface area.
- **Positive:** Preserves the strongest Obsidian pattern: layout leaves move
  independently from view content.
- **Positive:** Avoids treating current main views, side panels, and
  presentation modes as the same object.
- **Positive:** Keeps the current dense multi-agent shell recognizable while
  creating a path toward more malleable layouts.
- **Positive:** Allows Garden and Queue, or a plugin workbench and an agent
  session, to sit side by side without inventing nested global modes.
- **Positive:** Gives plugins meaningful layout participation without letting
  plugin count overwhelm the global titlebar.
- **Positive:** Preserves the current roster/sidebar strength by treating
  Cohorts as Wardian's agent working-set selector.
- **Positive:** Gives Garden a clear ecological structure: Habitat overview,
  Sites as places, and Cohorts as situated agent groups.
- **Negative:** Requires a model migration from `viewMode` and the existing
  `useLayoutStore` shape.
- **Negative:** Renaming Watchlists to Cohorts requires careful migration,
  docs updates, and backwards-compatible persisted state handling.
- **Negative:** Some current views may need to split into perspective,
  auxiliary-panel, and surface implementations.
- **Negative:** A real `file-viewer` and browser surface require stronger
  lifecycle, focus, and persistence handling than placeholder panels.
- **Negative:** Introduces more vocabulary. Product copy and developer APIs
  must make the difference between perspective, layout context, agent
  workspace, Site, Cohort, panel, surface, and mode obvious.
- **Negative:** Plugin contribution governance becomes part of the layout
  system; otherwise plugins can degrade navigation quality.

## Additional Considerations

### Persistence and Migration

Layout state should be versioned from the start. The first implementation can
keep most state in the frontend store, but the schema should be backend-readable
eventually so agents, CLI commands, and Markdown-backed project state can
inspect open layout contexts and surface handles. Migration should map the
existing `viewMode` values into core `PerspectiveDefinition` entries and likely
turn the current topbar choices into default pinned entries.

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
layout_context_id
surface_leaf_id
surface_view_id
auxiliary_panel_id
perspective_definition_id
```

`workspace_id` or `workspace_path` should refer only to the agent/project
workspace, never the UI layout context.

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

Small screens should emphasize perspective/layout-context picking and one
active surface at a time. Splits can collapse into tab stacks, side docks can
become drawers, and Home can become more important as a launcher. The data
model should survive these responsive transforms without creating a separate
mobile layout concept.

### Testing and Rollout

The first implementation plan should include schema snapshot tests, migration
tests from the existing layout store, browser E2E coverage for opening,
splitting, tabbing, promoting, and restoring surfaces, and screenshot evidence
for the changed navigation states. Native tests become necessary once terminal
or provider session lifecycle claims are part of the change.

Existing Watchlist persistence should migrate into Cohort language without
breaking stored data or CLI compatibility. The implementation can keep legacy
field names internally for one migration slice if needed, but the v2 UI should
not continue exposing Watchlist as the product term.

## Open Questions

- Which first slice proves the model best: command-center Site, workflows
  workbench, or agent-graph plus queue side-by-side?
- What should Home contain in v2: pinned perspectives only, recent Sites,
  active agent sessions, saved layouts, plugin templates, or all of these?
- Is Site the right user-facing term for `LayoutContext`, or should the product
  test Habitat, Plot, or another ecological label before implementation?
- Should every Cohort switch restore a Site, or should Wardian also support
  lightweight temporary Cohort filtering within the current Site?
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
