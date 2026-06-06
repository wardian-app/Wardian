# Malleable Garden Design Philosophy

- **Status:** Proposed
- **Date:** 2026-06-02

## Context and Problem Statement

Wardian is already moving toward malleable software: agents, classes, skills,
workflows, workflow runs, docs, and runtime evidence are visible as local files
or backend-owned records rather than hidden inside a sealed application. The
future Garden view should make that malleability concrete. It should become the
spatial habitat where users arrange agents, workflows, memories, skills, files,
and evidence around the work they are doing.

The design inspiration is Ink & Switch's essay
[Malleable Software](https://www.inkandswitch.com/essay/malleable-software/).
The most relevant principles for Wardian are:

- users should have a gentle slope from using a system to shaping it;
- tools should compose over shared data rather than create disconnected silos;
- customization should happen in place, while the user is working;
- local groups should be able to evolve their own working environments.

Wardian's challenge is to support this without turning Garden into a second
source of truth. Existing surfaces already own important state:

- the Rust backend owns agent lifecycle, PTY state, live delivery, and telemetry;
- `watchlists/index.json` owns watchlists and current team records;
- `library/workflows/` owns markdown-backed workflow blueprints;
- `logs/workflows/` owns durable workflow run evidence;
- the Library owns prompts, classes, and skills;
- Explorer and provider runtimes operate against real workspaces and folders.

Garden should connect and arrange those things, not duplicate them.

## Proposed Decision

Garden will be a spatial, scope-aware lens over canonical Wardian entities. It
will own layout and local annotations, while real mutations continue to go
through the same backend commands and filesystem records used by Grid, Library,
Workflows, Explorer, Watchlists, Queue, and the CLI.

### Canonical Entity References

Garden nodes should point at canonical Wardian objects using stable references:

```ts
export interface EntityRef {
  kind:
    | "agent"
    | "class"
    | "team"
    | "watchlist"
    | "workspace"
    | "folder"
    | "workflow"
    | "workflow_run"
    | "skill"
    | "prompt"
    | "memory"
    | "artifact"
    | "queue_item"
    | "garden_scene";
  id: string;
  source: "backend" | "wardian_home" | "workspace" | "library" | "logs";
  workspace_id?: string;
  path?: string;
  version?: string;
}
```

These references are identity handles. Garden may cache display labels for
rendering, but every important action should resolve the reference back through
the owning command or file boundary before applying a change.

### Garden-Owned State

Garden-owned state is intentionally narrow:

- spatial position and dimensions;
- visual grouping;
- pinned or hidden state;
- local notes and labels;
- color or shape hints;
- collapsed or expanded state;
- saved scene filters;
- relationship visibility preferences.

Garden must not own:

- agent process state;
- provider runtime state;
- team membership as the canonical record;
- skill deployment as the canonical record;
- workflow blueprint contents;
- workflow run results;
- durable memory contents;
- workspace or folder contents.

For example, dragging a skill onto an agent in Garden may initiate a skill
deployment, but the deployment is performed by the Library/backend command. The
Garden scene records the visual relationship only after the canonical mutation
succeeds.

### Scopes as Lenses

Wardian should not collapse scope into a single hierarchy. The same real object
can participate in multiple scopes at once:

| Scope | Meaning |
| --- | --- |
| `workspace` | Filesystem execution boundary where commands run. |
| `folder` | Physical subtree where materials live. |
| `team` | Durable project or workstream intent scope. |
| `watchlist` | Monitoring and targeting lens for what should be visible now. |
| `agent` | Actor/session boundary. |
| `class` | Reusable capability and instruction template. |
| `workflow` | Reusable process template. |
| `workflow_run` | Runtime instance and evidence boundary. |
| `memory` | Provenance-backed knowledge scope. |
| `garden_scene` | Spatial arrangement of references and relationships. |

This keeps Wardian honest about the relationship between work and files. A
single project can span multiple folders or workspaces, and a single workspace
can contain several active projects. Garden should show those overlaps instead
of forcing users into one tree.

### Team as Project or Workstream Scope

The existing `team` primitive should be interpreted as a durable intent scope,
closer to how many applications use "project" than to a simple roster group.
The current user-facing label can remain `Team`, but product and architecture
work should treat teams as workstreams that gather:

- agents;
- workflows;
- workspace and folder references;
- skills and prompts;
- memories;
- workflow runs;
- queue evidence;
- artifacts.

This implies a many-to-many model:

- one team can span many workspaces and folders;
- one workspace or folder can participate in many teams;
- one agent can work on multiple teams over time, even if current watchlist
  membership rules keep active roster grouping simpler;
- one workflow can be reused across several teams;
- memories and artifacts can be promoted into a team/project context without
  becoming tied to a single agent.

Garden should make this overlap visible. It should not visually nest teams under
folders or folders under teams as the default ontology.

### Relationship Edges

Garden should expose typed relationships instead of relying on spatial
proximity alone:

- `contains`
- `watches`
- `member_of`
- `uses_skill`
- `deployed_to`
- `runs`
- `produced`
- `evidence_for`
- `assigned_to`
- `derived_from`
- `depends_on`

Some relationships are canonical and owned elsewhere, such as team membership
or workflow run evidence. Others are Garden-local, such as a user drawing a
planning relationship between a memory and a future workflow. The persistence
format should distinguish canonical edges from Garden-local annotations.

## First Product Slice

The first Garden implementation should feel like a living command map rather
than a general-purpose visual programming environment.

### In Scope

- Create and open named Garden scenes.
- Add existing agents, teams, watchlists, workspaces, folders, workflows,
  workflow runs, skills, prompts, memories, artifacts, and queue items to a
  scene by reference.
- Move, group, pin, collapse, annotate, and filter those references.
- Show canonical relationship edges and selected Garden-local annotation edges.
- Launch existing actions from selected nodes through existing command paths.
- Save scenes as inspectable Wardian files under the active Wardian home.

### Initial High-Value Scenes

1. **Project habitat:** a team/workstream surrounded by its agents, folders,
   workflows, memories, and recent evidence.
2. **Capability map:** an agent or class connected to its skills, prompts,
   workspace, memory scopes, and active work.
3. **Workflow habitat:** a workflow blueprint connected to its roles, target
   teams, required skills, recent runs, outputs, and promoted memories.

These scenes are useful even before deeper programmability exists. They also
create the gentle slope: users start by arranging and annotating, then graduate
to small canonical actions, then eventually to generated or user-authored tools.

## Deferred Scope

The following are intentionally deferred until the spatial reference model is
stable:

- arbitrary user-authored Garden widgets;
- full visual programming over Garden nodes;
- multi-user realtime collaboration or CRDT scene sync;
- AI-driven auto-layout as the primary behavior;
- a separate graph database;
- making filesystem folders the universal organizing model;
- replacing Grid, Library, Workflows, Explorer, Queue, or Watchlists.

AI assistance should appear first as suggestions and previews: arrange by
status, propose missing links, summarize a team habitat, promote run output to a
memory proposal, or draft a workflow from a visible scene. Users should be able
to inspect and accept the resulting changes.

## Persistence Direction

Garden scenes should be inspectable on disk. A future implementation may choose
markdown with frontmatter plus a structured JSON block, or a small markdown file
paired with adjacent layout JSON. Either way, scene files should use portable
paths and `EntityRef` records instead of local-machine-only absolute paths when
the referenced object has a stable Wardian identity.

The persistence format should support:

- scene metadata;
- versioned layout records;
- entity references;
- Garden-local annotations;
- Garden-local relationship edges;
- filters and display preferences;
- migration between schema versions.

## Consequences

- **Positive:** Garden becomes Wardian's malleability layer without weakening
  backend authority over live runtime state.
- **Positive:** Teams gain a clearer product meaning as project/workstream
  scopes that can cross workspace boundaries.
- **Positive:** Users can compose agents, workflows, skills, memories, and
  evidence around the work itself.
- **Positive:** Future AI-generated tools become more useful because they can
  operate inside an existing Wardian habitat with persistence and provenance.
- **Negative:** The model requires stable entity identity before the first
  Garden implementation can feel reliable.
- **Negative:** Team/project semantics are broader than the current
  watchlist-backed team storage, so implementation must separate durable intent
  scope from roster display preferences over time.
- **Negative:** Garden-local edges and canonical edges need clear UI treatment
  so users can tell planning annotations from real system relationships.

## Implementation Seeds

Future implementation planning should start with these seams:

- add a shared `EntityRef` DTO in the frontend and Rust/core model layer;
- add read-only commands for resolving entity references into display metadata;
- persist Garden scenes under the active Wardian home;
- expose existing watchlist teams as team/project entity references;
- keep team membership mutations routed through watchlist/team commands until a
  dedicated project-scope model exists;
- extend memory promotion work so memories can attach to team/project scopes,
  not only agents or workflow runs;
- add tests that prove Garden scene persistence does not duplicate or mutate
  canonical agent, workflow, skill, or memory records.
