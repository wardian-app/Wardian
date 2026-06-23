# Garden Preliminary Implementation

- **Status:** Accepted
- **Date:** 2026-06-23
- **Related:** `2026-06-02-malleable-garden.md`, `2026-06-07-navigation-layout-v2.md`,
  epic [#513], issue [#520]

## Context

Garden is currently a `PlaceholderView` ("coming in Phase 5") wired into the
`viewMode` tab bar. The two design specs above describe an ambitious end state:
a spatial operating surface over the full Habitat primitive graph, eventually
hosted inside the HabitatLayout Site/Cohort/Perspective rework (#513).

That end state is too large for a first slice, and #513 has not started. This
spec defines a **preliminary, standalone Garden** that ships real value now,
inside the existing shell, while staying shaped so the later HabitatLayout
migration is a re-host rather than a rewrite.

The driving metaphor (from the navigation v2 spec) is "closer to a canvas or
strategy game than a configuration form": agents are units you grab and move
around a spatial field.

## Decisions

| Area | Decision | Rationale |
| --- | --- | --- |
| **Scope** | Standalone view in the current `viewMode` shell. No Site/Cohort/Perspective work. | Fastest path to a usable Garden; HabitatLayout absorbs it later. #513 is unstarted. |
| **Rendering** | `react-konva` (Canvas 2D scene graph). | Lighter than Pixi at our scale (dozens of units), redraws only on change (protects idle CPU), crisp native text for labels, built-in hit-testing + drag events. Wardian entities stay the source of truth (controlled rendering), unlike tldraw which owns its own document store and ships a license watermark. |
| **Units rendered** | Agents + workflows only. | The two primitives that read as autonomous "units". Other EntityRef kinds (skills, memories, artifacts, …) deferred. |
| **Unit visual** | Living orb / cell. Agent = circular orb with a status-colored aura. Workflow = segmented pod showing run pips. | Ecological/strategy-game identity; distinct silhouettes keep the two kinds legible at a glance. |
| **Topology** | Free-floating units, **no** team/cohort container regions. | Matches the RTS-unit mental model; container regions belong to the later Site model. |
| **Edges** | Seed-only placement hints, not the primary paradigm. Agent relations come from the existing `graphProjection`. | Keeps Garden distinct from Graph (sigma topology). Edges may render faintly but are not the point. |
| **Workflow placement** | Cluster all workflows into one band/shelf region of the canvas. | Workflows have no edges in `graphProjection` (it is agent↔agent only), so they cannot be relation-seeded. A calm "workflow shelf" is the simplest honest v1. |
| **Animation** | Animate **only** units that are actively processing (cyan). Idle/off/error units are static. | A living-orb that always pulses defeats Konva's redraw-on-change idle efficiency and Wardian's idle-CPU goals. Motion = attention is also more legible. |
| **Persistence** | `useGardenStore` via zustand `persist` → `localStorage`. Stores per-unit drag positions and pins only. | Same pattern as `useLayoutStore`. Survives reload + app restart, zero backend work. Disk-backed scene files (per the malleable-garden spec) are a later slice. |

## Architecture

### Data flow (reuse, do not rebuild)

`buildAgentGraph` in `src/features/graph/graphProjection.ts` already produces,
from inputs App.tsx assembles for GraphView:

- `nodes`: agent nodes with seeded `x`/`y`, derived `status`, `color`,
  `size`, `recent`, and the underlying `AgentConfig` + `telemetry`;
- `edges`: relation edges (`same_team`, `shared_workspace`, `same_worktree`);
- `clusters`, `visibleAgents`, `scopeLabel`.

Garden v1 consumes the **same projection** and renders it with Konva instead of
Sigma. This means:

- agent status/color logic is reused, not duplicated;
- initial agent positions are the projection's seeded `x`/`y`;
- the Garden store overlays user drag positions on top of the seed.

```text
App.tsx (same props as GraphView)
  -> buildAgentGraph(...)            // existing projection: agent nodes + seed x/y + edges
  -> useWorkflowLibrary / workflow list  // workflow units (id, name, run state)
       |
       v
  GardenView
    -> useGardenStore (positions, pins; persisted)   // drag overrides on top of seed
    -> GardenCanvas (react-konva)
         Stage > Layer
           AgentUnit[]      // orb + status aura, animate only when processing
           WorkflowUnit[]   // segmented pod + run pips, clustered shelf
           (faint relation edges, optional)
```

### New files (proposed)

```text
src/features/garden/
  gardenProjection.ts     // adapt AgentGraphProjection + workflows -> GardenScene units
  GardenCanvas.tsx        // react-konva Stage/Layer; pan/zoom; selection; drag
  AgentUnit.tsx           // Konva group: orb, aura, label, status; pulse when active
  WorkflowUnit.tsx        // Konva group: segmented pod, run pips, label
  garden.types.ts         // GardenUnit, GardenScene, GardenPosition
src/store/useGardenStore.ts   // zustand + persist: { positions, pins }
src/views/GardenView.tsx      // replaces PlaceholderView for viewMode === "garden"
```

### State ownership boundary (from malleable-garden spec)

Garden owns **only** layout: drag positions and pins. It must not own agent
lifecycle, telemetry, provider state, workflow contents, or run results — those
are read from existing stores/commands. Opening full session inspection routes
to the existing Grid/agent-session path (mirror GraphView's
`onOpenAgentInGrid`), not an embedded terminal.

### HabitatLayout-forward shaping

To keep the later #513 migration cheap without building it now:

- name the unit type `GardenUnit` carrying an `EntityRef`-shaped `{ kind, id }`
  so agents and workflows are addressed by reference, not ad-hoc fields;
- keep `GardenCanvas` a pure render-from-props component so it can later be
  hosted as a `habitat-canvas` surface inside a Site without rework;
- keep persistence keyed by stable entity id, so a future disk-backed scene
  file can adopt the same position map.

## In scope (v1)

- Replace the Garden placeholder with a real `GardenView`.
- Render agent units (live status/color from the existing projection) and
  workflow units (clustered shelf).
- Pan, zoom, select, and drag units; persist positions across restart.
- Pulse animation only on actively-processing agents.
- Open an agent into Grid from its unit (reuse GraphView's routing).

## Out of scope (deferred)

- Site/Cohort/Perspective containers and the HabitatLayout migration (#513).
- Other entity kinds (skills, prompts, memories, artifacts, MCPs, classes).
- Disk-backed markdown+JSON scene files and CLI inspection.
- Garden-authored relationship edges / annotations.
- Workflow→agent placement from run records (needs run↔agent linkage first).
- Rich per-agent live previews; full terminal/transcript embedding.
- Pixi/WebGL shader-driven ambient visuals.

## Verification

- Frontend unit tests for `gardenProjection` (agent+workflow → units, position
  overlay), `useGardenStore` (persist round-trip), and unit components.
- Browser E2E: Garden tab renders units, drag persists across reload, clicking
  an agent unit routes into Grid. (Layer: Browser E2E — no PTY/native claims.)
- Screenshots of the populated Garden field and an active (pulsing) agent unit
  for the PR per the screenshot rule.

## Consequences

- **Positive:** A usable, on-brand Garden ships without waiting on #513.
- **Positive:** Heavy reuse of `graphProjection` keeps status logic single-source.
- **Positive:** Idle-CPU-friendly by construction (redraw-on-change + animate
  only active units).
- **Negative:** localStorage persistence is opaque (not CLI/markdown
  inspectable); a later slice must migrate to disk-backed scenes.
- **Negative:** Workflow units are relation-less in v1 (clustered shelf only)
  until run↔agent linkage exists.
- **Negative:** A second renderer (Konva) joins Sigma in the bundle; acceptable
  for the distinct interaction model, revisit if footprint matters.

[#513]: https://github.com/wardian-app/Wardian/issues/513
[#520]: https://github.com/wardian-app/Wardian/issues/520
