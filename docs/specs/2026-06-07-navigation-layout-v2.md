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
  -> creates or updates a Site

Site
  -> owns ShellRegions and a SurfaceCanvas

SurfaceCanvas
  -> owns SurfaceLeaves

SurfaceLeaf
  -> hosts one SurfaceView

SurfaceView
  -> owns SurfaceModes and SurfaceRegions
```

A **Site** is a saved operating place inside Wardian's Habitat: active
perspective lineage, surface canvas, dock state, and active Cohort. It should
be the product term and the internal domain term. Earlier drafts used
`LayoutContext`; v2 should use `Site` in product copy, APIs, stores, persisted
schema, and command handles to avoid unnecessary translation.

```text
Habitat
  -> whole Wardian environment and primitive graph

Site
  -> saved/focused operating place inside the Habitat

Cohort
  -> reusable group of agents associated with or currently occupying a Site

Surface
  -> movable work object inside a Site

Perspective
  -> specialized way of viewing and working with Habitat primitives
```

Garden is not a hierarchy parent. It is the core Wardian perspective over the
Habitat primitive graph. It can show all major primitive types together and
their relationships, including agents, Sites, Cohorts, skills, prompts, MCPs,
classes, workflows, events, files/artifacts, memories, telemetry, and provider
state. It is also an operating surface: users should be able to directly
arrange, inspect, connect, and route work from it.

```text
Garden perspective
  -> all Habitat primitives and relationships

Agents perspective
  -> agents; current Grid behavior becomes its default layout mode

Dashboard perspective
  -> aggregate telemetry, health, and status across agents, providers, runs,
     and events

Graph perspective
  -> agent communication and rule topology: who can talk to whom, through what
     channels, and under what constraints

Queue perspective
  -> events/signals/actions

Library perspective
  -> skills, prompts, MCPs, classes, workflows, and reusable catalog objects
```

This makes Garden cleaner: an all-Habitat Garden can show multiple Sites, each
with Cohorts arranged around or inside them. A Cohort-focused Garden can focus
one Site for work. Queue remains a perspective, not a primitive or an inbox;
the underlying primitives are events/signals/actions.

Garden and Graph are separate perspectives. Garden is the spatial operating
view of the Habitat: where agents, Sites, Cohorts, reusable primitives,
artifacts, events, telemetry, memories, and provider/runtime state become
inspectable and movable. Graph is narrower: it focuses on communication,
dependency, and rule topology between agents and related primitives.

Alternatives considered:

- `Workspace`: rejected because it conflicts with agent workspaces.
- `Habitat`: too broad; it should remain the larger Wardian environment.
- `Station`: too control-room oriented for Wardian's ecological theme.
- `Desk`: familiar, but too generic and less ecological.
- `Lens`: useful metaphor for filtering, but too narrow for a full saved
  layout.
- `Deck`: tactile, but can imply cards/trading and is less obvious than
  Site.

The internal namespace for this layer should be **HabitatLayout**, not
`LayoutV2`. `HabitatLayout` names the UI arrangement layer for Sites,
perspectives, surfaces, docks, and Cohort focus. It does not own agent
lifecycle, provider state, project workspaces, or the full Habitat primitive
graph.

The system should use **Extensions** consistently for third-party or optional
contributions. Do not split product and internal naming into separate terms
unless a low-level package format absolutely requires it.

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
definition or template: a saved, predefined, extension-contributed, or user-created
arrangement of shell regions, auxiliary panels, and central surfaces.

The live object is the Site. A Site may be created from a perspective
definition, may keep a base perspective identity for orientation, and may then
diverge as the user opens surfaces, splits leaves, switches Cohorts, and pins
tools.

Do not call this object a workspace. In Wardian, an agent workspace already
means the target project directory or worktree where the provider runs.
Overloading that term would blur the difference between "where an agent is
working on disk" and "how the user has arranged the Wardian UI."

```text
PerspectiveDefinition
  -> describes the kind of work and default layout

Site
  -> live/saved UI context created from or associated with a perspective

SurfaceTree
  -> actual split/tab graph inside the Site
```

Current main views map first to perspective definitions, not directly to
primitive surface types:

```text
PerspectiveDefinition:
  - agents              // current grid lineage; specializes in agents
  - dashboard           // current dashboard view; telemetry/status/health
  - graph               // current graph view; communication/rule topology
  - queue               // current queue view; specializes in events/signals
  - workflows           // current workflows view
  - library             // current library view
  - garden              // core habitat graph perspective
  - extension-defined   // extension-contributed perspectives
```

The titlebar/top tab area should show open surfaces in the active Site, not a
second set of launcher tabs. Perspective discovery belongs in Home/New Surface
and quick open.

### 3. Navigation Model

Wardian should use an Obsidian/JupyterLab-style model: tabs represent open
surfaces inside the active Site. Perspectives can open as surfaces, but the
top bar is not a fixed list of global perspective launchers.

```text
Startup
  -> restore the last active Site; first-run behavior can use Home later

Right sidebar
  -> switch Cohorts/Sites

Top tabs
  -> open surfaces and perspective-surfaces in the active Site

+ tab
  -> open Home/New Surface picker

Home/New Surface
  -> choose a perspective, surface type, or extension contribution

Surface tabs and splits
  -> manage the open work objects inside the active Site

Command palette / quick open
  -> open or focus any perspective, Site, surface, panel, or command
```

The current global topbar tabs are useful for a small fixed product, but they
scale poorly once extensions can contribute surfaces and perspectives. Home
should not become a mandatory hub that users pass through during normal work.
It is a first-run, empty-Site, recovery, and new-surface picker.

Recommended model:

```text
Right sidebar answers: which Cohort/Site am I working in?
Top tabs answer:      which surfaces are open in this Site?
+ / Home answers:     what should this new tab become?
Quick open answers:   where can I jump immediately?
```

The `+` picker should group entries by kind:

```text
Perspectives
  - Garden
  - Agents
  - Dashboard
  - Graph
  - Queue
  - Library
  - Workflows

Surfaces
  - Agent session
  - File
  - Terminal
  - Browser

Extensions
  - extension-contributed perspectives and surfaces
```

Do not include auxiliary panels such as Source Control or Extensions in the
default surface picker. They live in the left dock and route users into central
work surfaces.

The target tab model is central pane-local tabs:

```text
SurfaceCanvas
  -> split panes
    -> each pane owns a local tab stack
    -> tabs can move between central panes
    -> tabs can split left/right/up/down
```

An early epic child issue may start with one central tab stack. Later child
issues should add split panes and pane-local tabs. Wardian should not initially
allow arbitrary tab movement into the left, right, or bottom docks; those docks
remain auxiliary regions. Auxiliary panels route to central work surfaces rather
than moving wholesale into the surface canvas.

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

Site
  -> saved UI arrangement, active perspective lineage, open surfaces, docks,
     and active Cohort/filter state
```

The right roster sidebar is therefore more than a passive list. It is the active
working-set selector for many Wardian tasks. HabitatLayout should let a Site
remember the active Cohort, visible teams, and collapsed team state. It
should not persist selected agents, focused surface, or generic scroll state in
the initial HabitatLayout work; selected agents and focus are intertwined interaction state,
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

### 5. Garden Operating Model

Garden is both a perspective and an operating surface. It spatially shows the
full Habitat plan:

- agents
- Sites
- Cohorts
- skills
- prompts
- MCPs
- classes
- workflows
- files/artifacts
- events/signals
- telemetry
- memories
- provider/runtime state

Sites should appear as containers or regions, not only as nodes. Cohorts and
agents can be arranged inside or around Sites.

Garden interactions should feel closer to a canvas or strategy game than a
configuration form:

- drag agents into Sites or Cohorts
- rearrange Cohort placement
- connect or inspect relationships
- open related surfaces
- trigger simple contextual actions
- visually monitor state

Detailed configuration stays in dedicated panels and surfaces such as Agent
Config, Extensions, and Library editors. Garden should expose direct,
spatially meaningful actions, not replace every specialized editor.

Garden should not show raw terminal/session content by default. Agent nodes can
show compact live summaries, latest status/thought/event, and attention
markers. A focused agent can show a richer preview when useful, but full
terminal, chat, transcript, or session inspection should open an `agent-session`
surface in the Site.

### 6. Site Persistence Boundary

A Site should remember the parts of the operating setup that make switching
between working sets feel seamless:

```text
Site remembers:
  - base perspective definition
  - surface canvas tree
  - open surface views and their surface-owned state
  - shell region and dock sizes/collapsed state
  - active Cohort
  - visible teams and collapsed team state
  - user pins relevant to the Site
```

A Site should not remember these in the initial HabitatLayout work:

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

### 7. Recovery and Reset

HabitatLayout should recover to continuity first and reset only on explicit
user action. Normal startup restores the last active Site. Home/New Surface is
the fallback when there is no valid last Site, when the first Site has not been
created, or when saved layout state cannot be restored cleanly.

Recovery must be non-destructive to runtime state. A broken layout should not
kill agents, stop PTYs, delete provider sessions, clear events, remove
workspaces, or mutate Cohorts. It should only affect the view arrangement unless
the user invokes a separate lifecycle command.

The reset model should be scoped:

```text
Reset surface view
  -> restore one surface's view-owned state
  -> does not destroy backend/runtime state

Close surface
  -> remove one tab/leaf from the Site
  -> does not kill an agent, PTY, browser session, or provider session

Reset pane layout
  -> collapse splits and tab stacks to the active pane or perspective default
  -> preserve recoverable open surfaces in a simple tab stack

Reset Site
  -> restore shell regions, docks, and surface canvas from the base perspective
  -> preserve active Cohort unless the user explicitly changes it

Create fresh Site
  -> start from a perspective definition and active Cohort
  -> leave the previous Site intact

Reset HabitatLayout
  -> clear saved Sites, dock state, pins, and layout snapshots
  -> preserve agents, Cohorts, project workspaces, provider state, and Habitat
     primitives
```

If a saved Site references a missing surface type, disabled extension,
unavailable perspective, or invalid split tree, Wardian should show a recovery
placeholder instead of silently dropping the entry. The user should be able to
close the broken surface, reset the Site, open Home/New Surface, or inspect the
saved layout state on disk.

HabitatLayout should keep enough versioned state to recover from bad migrations
and partial writes. A future implementation can keep a last-known-good snapshot
or transaction log, but the product contract is simpler: the app should always
offer a usable Home/New Surface fallback and should never make users choose
between opening Wardian and preserving their agents.

### 8. Auxiliary Panels and Surface Routing

Auxiliary panels are docked tools. They live in left, right, or bottom regions
by default. They open or focus central work surfaces, but they are not primary
main surfaces by default. The initial HabitatLayout epic should not support
dragging whole auxiliary panels into the main surface area.

```text
AuxiliaryPanelType:
  - file-explorer
  - source-control
  - agent-config
  - spawn-agent
  - command-broadcast
  - workflow-glance
  - extensions
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
- `WorkflowsGlancePane` -> `workflow-glance`
- Extensions panel -> `extensions`
- `AgentWatchlist` -> `agent-roster`
- `UserTerminalPanel` -> `terminal-drawer` by default

The important correction is that the right agent roster is not a special main
surface. It is an auxiliary panel, structurally comparable to the left side
tools. It is more persistent because it is important for orientation.

Auxiliary panel routing should open work objects, not whole panels:

```text
Explorer
  -> opens file-viewer surfaces

Source Control
  -> opens diff / file-review surfaces

Agent Roster
  -> opens agent-session surfaces
  -> may open or focus Agents perspective for the active Cohort

Queue Glance
  -> opens or focuses Queue perspective surface

Workflow Glance
  -> opens workflow monitor, workflow runs, or workflow editor surfaces

Extensions
  -> opens extension-contributed surfaces
  -> Extensions manager remains in the left dock

Command Broadcast
  -> targets active Cohort or selected agents; no central surface by default

Agent Config / Spawn
  -> panel/dialog flow; no central surface by default

Class Manager
  -> folds into Library; opens class catalog/editor surfaces through Library
```

### 9. Main Surface Area

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
  - queue
  - extension-surface
```

Garden and Queue side by side should initially be represented as two surfaces
in the same `SurfaceTree`, for example `habitat-canvas` beside `queue`.
It should not require nested perspectives in the initial HabitatLayout epic. A
future `PerspectiveInstance` container could support that, but it should be
deferred until the simpler surface-tree model proves insufficient.

Extension surfaces should behave like local work objects. They can be opened,
tabbed, split, saved, and restored according to their declared capabilities.
They do not receive permanent global titlebar placement by default.

### 10. Surface Modes and Regions

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
  -> agents perspective containing agent-session surfaces; "grid" becomes a
     layout mode, not the perspective name

DashboardView
  -> dashboard perspective for aggregate telemetry, health, and status

GraphView
  -> graph perspective containing agent-graph surface; focuses on agent
     communication, dependency, and rule topology

QueueView
  -> queue perspective containing queue surface; specializes in events,
     signals, and action-needed states

GardenView
  -> garden perspective containing habitat-canvas surface; all-Habitat mode
     shows primitives, Site regions, Cohorts, and live summaries, while
     Cohort/Site modes focus spatial operation inside one Site

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
separate perspective definitions, live Sites, docked panels, and central
surface leaves.

```ts
type HabitatLayoutState = {
  version: 1;
  active_site_id: string;
  sites: SiteState[];
  perspective_definitions: PerspectiveDefinition[];
  pinned_entries: PinnedNavigationEntry[];
};

type PerspectiveDefinition = {
  id: string;
  name: string;
  source: "core" | "extension" | "user";
  description?: string;
  template: PerspectiveTemplate;
};

type PerspectiveTemplate = {
  shell?: Partial<ShellRegionState>;
  surface_canvas: SurfaceTreeTemplate;
  recommended_pins?: PinnedNavigationEntry[];
};

type SurfaceTreeTemplate = SurfaceTree;

type SiteState = {
  id: string;
  name: string;
  base_perspective_id?: string;
  active_cohort_id?: string;
  collapsed_team_ids?: string[];
  shell: ShellRegionState;
  surface_canvas: SurfaceTree;
};

type PinnedNavigationEntry =
  | { type: "site"; site_id: string }
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
  | QueueSurfaceState
  | ExtensionSurfaceState;
```

## Design Rules

- Perspectives define work arrangements; Sites hold live UI state.
- Perspectives are templates and starting points, not necessarily mutually
  exclusive full-screen pages.
- Auxiliary panels control, navigate, and inspect.
- Surfaces are movable work objects.
- Surface leaves own placement; surface views own content state.
- Runtime state belongs to backend/provider state, not tabs. Closing a surface
  closes the view unless the user invokes an explicit lifecycle command.
- Moving, splitting, or tabbing a surface is layout-only and must preserve
  surface state according to its lifecycle policy.
- Lifecycle policy attaches to concrete surface type or mode, not just to the
  parent perspective.
- Site layout changes should auto-save.
- Startup restores the last active Site. Home/New Surface is the fallback for
  first-run, empty-Site, and layout recovery states.
- Recovery and reset are layout-scoped by default. They must not kill agents,
  stop PTYs, delete provider sessions, clear events, remove workspaces, or
  mutate Cohorts unless the user invokes an explicit lifecycle command.
- Broken Sites and missing surface types should render recoverable placeholders
  instead of being silently dropped from persisted state.
- The top tab bar represents open surfaces in the active Site, not pinned
  perspective launchers.
- The target model is central pane-local tabs. An early epic child issue may
  start with one central tab stack before adding split panes.
- Home/New Surface launches perspective definitions, surface types, recent
  Sites, and extension contributions. It should not be a mandatory route
  between normal tasks.
- Extensions live consistently under the Extension name. The Extensions panel
  belongs in the left dock by default; extension-contributed surfaces can open
  in the central surface canvas.
- Sites remember active Cohort and collapsed team state. They should
  not persist selected agents, focused surface, or generic scroll state in the
  initial HabitatLayout work.
- Cohort switching filters visible/targetable agents. It must not turn
  agents on/off or mutate provider lifecycle state.
- Switching between materially different working sets should usually switch the
  whole Site, not only apply a temporary cohort filter.
- Garden can render an all-agents Habitat view as multiple Sites arranged with
  their Cohorts, while a Cohort-focused Garden opens one Site.
- Garden is a perspective and operating surface, not a parent container. It
  should expose spatial actions while routing detailed editing to dedicated
  panels or surfaces.
- Garden shows Sites as containers/regions and should use compact agent
  summaries by default; full chat, terminal, transcript, and session inspection
  opens an `agent-session` surface.
- Graph focuses on communication, dependency, and rule topology. It should not
  become the full spatial Habitat view.
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
- **Positive:** Allows Garden and Queue, or an extension workbench and an agent
  session, to sit side by side without inventing nested global modes.
- **Positive:** Gives extensions meaningful layout participation without
  letting extension count overwhelm the global titlebar.
- **Positive:** Avoids duplicate top navigation by making the top tabs the
  actual open surfaces, while Home/New Surface handles launching.
- **Positive:** Preserves the current roster/sidebar strength by treating
  Cohorts as Wardian's agent working-set selector.
- **Positive:** Gives Garden a clear ecological structure: Habitat overview,
  Sites as places, and Cohorts as situated agent groups.
- **Positive:** Gives users a clear escape hatch from broken layouts without
  conflating UI reset with runtime or workspace deletion.
- **Negative:** Requires a model migration from `viewMode` and the existing
  `useLayoutStore` shape.
- **Negative:** Renaming Watchlists to Cohorts requires careful migration,
  docs updates, and backwards-compatible persisted state handling.
- **Negative:** Some current views may need to split into perspective,
  auxiliary-panel, and surface implementations.
- **Negative:** A real `file-viewer` and browser surface require stronger
  lifecycle, focus, and persistence handling than placeholder panels.
- **Negative:** Introduces more vocabulary. Product copy and developer APIs
  must make the difference between perspective, Site, agent workspace, Cohort,
  panel, surface, and mode obvious.
- **Negative:** Extension contribution governance becomes part of the layout
  system; otherwise extensions can degrade navigation quality.

## Additional Considerations

### Persistence and Migration

Layout state should be versioned from the start. The epic breakdown should
decide whether early persistence stays frontend-only or starts
backend-readable, but the schema should eventually be inspectable by agents,
CLI commands, and Markdown-backed project state. Migration should map the
existing `viewMode` values into core `PerspectiveDefinition` entries and
convert the current topbar choices into perspective surfaces or Home/New
Surface entries.

### Deferred Extension Governance

Extension governance is out of scope for this exploratory spec. The current
document only reserves the naming and placement boundary: Extensions can
contribute surfaces, auxiliary panels, commands, perspective definitions, and
suggested placements, but detailed contribution review, trust, versioning,
permissioning, and marketplace behavior should be handled by a separate spec or
epic.

Extensions should declare what they contribute:

```text
ExtensionContribution:
  - surface type
  - auxiliary panel type
  - command
  - perspective definition
  - suggested default placement
```

Core Wardian should validate contribution IDs, icons, labels, default
placement, and persistence payloads. An extension should not be able to
silently reserve global navigation, override core perspectives, or store
opaque layout state that cannot be migrated.

### Command and Addressing Model

Every open work object needs a stable handle. CLI commands, agents, command
palette actions, and future automation should be able to address:

```text
workspace_id
site_id
surface_leaf_id
surface_view_id
auxiliary_panel_id
perspective_definition_id
```

`workspace_id` or `workspace_path` should refer only to the agent/project
workspace, never the UI Site.

This is especially important for multi-agent workflows where an agent might
open a file viewer, focus a running terminal, request a browser surface, or
place a Queue surface beside the current session.

### Surface Lifecycle and Fidelity

Terminals, browser surfaces, file viewers, and agent sessions have different
mount/unmount costs. The layout system needs explicit lifecycle policies for
hidden tabs, background splits, suspended surfaces, restored PTYs, dirty files,
and provider sessions. Smooth malleability will fail if moving a leaf destroys
terminal scrollback, browser state, or file cursor state.

Runtime ownership should stay with backend/provider state, not with tabs.
Agents, PTYs, provider sessions, event streams, and agent workspaces continue
to exist outside any particular surface. Closing a surface closes the view; it
does not kill an agent, stop a PTY, delete an event, or destroy provider state
unless the user invokes an explicit lifecycle command.

Moving, splitting, or tabbing a surface is layout-only. It must preserve the
surface's view state according to that surface's lifecycle policy. Site layout
changes should auto-save so switching Sites feels continuous rather than
document-like.

Hidden heavy surfaces may suspend UI rendering, but their backend/runtime state
must remain alive. This is especially important for terminals, agent sessions,
browser sessions, and file edits.

Lifecycle policy attaches to concrete surface types or modes, not only to a
perspective:

```text
persistent-runtime
  - agent-session
  - user-terminal

stateful-view
  - browser
  - file-viewer
  - workflow-editor
  - skill-editor
  - prompt-editor
  - class-editor
  - mcp-editor

recomputable-view
  - garden
  - agents overview
  - dashboard
  - graph
  - queue
  - library catalog
  - workflow catalog
  - workflow runs
  - workflow monitor
```

Perspective surfaces are usually disposable/recomputable views over Habitat
state. Editors are the exception: a Library or Workflows perspective can open
stateful editor modes, but the catalog/monitor views themselves should be
recomputed from durable state.

### Accessibility and Keyboard Use

The v2 model needs keyboard-first navigation from the beginning: quick switcher,
surface focus traversal, split movement, tab movement, dock toggles, and
surface-opening commands from auxiliary panels. Surface tabs and dock selectors
should map to real ARIA tab/menu patterns rather than custom click-only
controls.

### Responsive Layout

Small screens should emphasize perspective/Site picking and one active surface
at a time. Splits can collapse into tab stacks, side docks can become drawers,
and Home can become more important as a launcher. The data model should survive
these responsive transforms without creating a separate mobile layout concept.

### Testing and Rollout

The GitHub epic should cover the full HabitatLayout direction rather than a
single narrow first slice. Child issues should still be staged, but the epic
needs to preserve the end-to-end model: Sites, Cohorts, perspectives, central
tabs/splits, auxiliary-panel routing, surface lifecycle, recovery/reset,
migration, and testing.

The implementation plan should include schema snapshot tests, migration tests
from the existing layout store, browser E2E coverage for opening, splitting,
tabbing, auxiliary-panel routing, resetting, and restoring surfaces, and
screenshot evidence for the changed navigation states. Native tests become
necessary once terminal or provider session lifecycle claims are part of the
change.

Existing Watchlist persistence should migrate into Cohort language without
breaking stored data or CLI compatibility. The implementation can keep legacy
field names internally for one migration slice if needed, but the v2 UI should
not continue exposing Watchlist as the product term.

## Epic Scoping Notes

No major product-taxonomy questions remain in this exploratory spec. The next
step is to scope the whole direction into a GitHub epic with staged child
issues.

The epic should turn these choices into implementation work:

- define the `HabitatLayoutState`, `SiteState`, and migration path from current
  `viewMode` and watchlist state
- build Home/New Surface as the first-run, launcher, empty-Site, and recovery
  fallback
- replace global main-view tabs with open surface tabs inside the active Site
- implement central surface tabs first, then split panes and pane-local tabs
- route auxiliary panels into central surfaces without making panels draggable
  in the first pass
- implement Cohort-backed Site switching without mutating agent lifecycle
- implement Garden as a spatial operating surface and Graph as communication
  topology
- implement recovery/reset scopes for surface view, pane layout, Site, and
  HabitatLayout
- decide whether `user-terminal` remains bottom-dock-first or becomes a main
  surface during the epic breakdown
- decide whether HabitatLayout persistence starts frontend-only or
  backend-readable during the epic breakdown
- defer detailed Extension governance to a separate spec/epic
