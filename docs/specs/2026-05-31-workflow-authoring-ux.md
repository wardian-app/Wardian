# Workflow Authoring UX Refresh

- **Status:** Implemented
- **Date:** 2026-05-31
- **Branch:** `debug/workflow-crash`
- **Scope:** Frontend authoring experience for workflow blueprints.

## 1. Problem

Workflow has the right backend direction: registry-backed node types, durable
blueprints, run logs, observe mode, schedules, and monitor surfaces. The current
authoring UI does not match that strength. It feels like a backend demo:

- a permanent left palette lists node names with little context;
- a permanent right panel consumes space even when it is not useful;
- a permanent runs drawer competes with authoring even in edit mode;
- node cards hide too much of the authored workflow;
- adding, duplicating, deleting, and inspecting nodes is less fluid than the old
  old workflow system builder;
- the canvas is visually dominant but functionally underused.

The old workflow system frontend had better authoring affordances, but its architecture and
workflow model should not be restored. This refresh restores the interaction
quality while keeping the workflow model intact.

## 2. Goals

Authoring mode should become a canvas-first workflow construction surface. The
node registry should drive discovery, node summaries, configuration forms,
validation states, IO hints, and insertion behavior.

This pass should:

- make node discovery searchable and descriptive;
- make nodes readable without opening the inspector;
- make the inspector contextual and collapsible;
- move run history out of the default edit-mode layout;
- add essential canvas and node actions;
- auto-layout workflows by default so agents and markdown authors do not need
  to provide coordinates;
- preserve workflow-only concepts: one manual entry node, external schedules, durable
  runs, and registry-defined node schemas.

## 3. Non-Goals

- Do not restore the old workflow system store or old workflow system frontend component cluster.
- Do not restore scheduled trigger or file watcher trigger nodes.
- Do not recreate old workflow system launch behavior.
- Do not make the builder a separate top-level route again.
- Do not introduce a new backend contract for this first authoring pass.

## 4. Recommended Product Shape

### 4.1 Edit Mode Layout

Edit mode should reserve most of the viewport for the canvas. The top toolbar
keeps blueprint selection, mode switch, workflow name/status, save, and run.

The fixed left palette is replaced by a compact **Add node** entry point and a
searchable block library. The library can open from:

- the toolbar add button;
- an empty-canvas call to action;
- right-click on the canvas;
- optionally, an edge or node quick action when inserting downstream nodes.

The right inspector becomes contextual. It opens for selected nodes, selected
edges, workflow metadata, or validation issues. When nothing is selected, it
collapses so the canvas can use the space.

The runs drawer is hidden by default in edit mode. Runs remain first-class in
Observe and Monitor; edit mode can expose a compact run strip or button if
needed, but not a permanent 280px drawer.

### 4.2 Registry-Backed Block Library

The block library is generated from `nodeRegistry.schema.json`, not an old workflow system block
list. Each item shows:

- label and category;
- short description;
- required fields count or key required fields;
- input and output port summary;
- dynamic output note when `outputs_from_field` is present;
- validation or availability notes when applicable.

Search should match label, category, description, field names, and port labels.
Categories are useful, but they should not be the only navigation path.

The library should support insertion from local context, but placement is still
owned by the builder. Newly inserted nodes enter the graph without requiring
authored coordinates; the canvas lays them out automatically.

### 4.3 Informative Node Cards

Node cards should carry the workflow's meaning, not just the node type.

Each card should show:

- node label or name;
- registry type/category;
- validation state;
- required field missing indicators;
- concise summaries of key authored fields;
- input/output port labels, especially branch, loop, and decision nodes.

The summary logic should be registry-aware and conservative. For example:

- `task`: agent/role plus a prompt preview;
- `shell`: command preview and working directory when set;
- `branch`: condition preview and True/False port labels;
- `loop`: max iterations or until condition, plus Body/Done labels;
- `manual_trigger`: input schema summary when present;
- `sub_workflow`: referenced workflow id.

Long prompts and schemas should be summarized, not rendered raw.

### 4.4 Contextual Inspector

The inspector should answer "what can I edit now?" It should not be a permanent
blank or low-value panel.

For a selected node, show:

- node identity and type description;
- required fields first;
- advanced fields collapsed by default;
- typed inputs derived from field kinds;
- variable assistant near fields that support templates;
- validation messages scoped to the node.

For a selected edge, show source, target, ports, and delete action. For workflow
metadata, show id/name and future workflow-level settings. For validation, show
issues with focus actions.

### 4.5 Canvas Actions

Restore high-value authoring actions from old workflow system as workflow-native interactions:

- right-click canvas: open block library at pointer;
- right-click node: duplicate, copy node id, delete, inspect;
- right-click edge: delete;
- fit view after blueprint load and after inserting templates;
- selected node quick actions for duplicate/delete;
- keyboard shortcuts where safe: Delete/Backspace for selected node or edge,
  Escape to close menus/drawers.

These actions operate on the workflow `Blueprint` shape and existing builder store.

### 4.6 Default Auto Layout

Auto layout is the default authoring behavior, not an optional enhancement or a
button-first workflow. The engine model remains the graph: nodes and edges.
Canvas positions are optional editor metadata.

The builder should compute positions for the default view even when older
blueprints include persisted coordinates. Agents and markdown-authored workflows
should be valid and readable without any `position` fields, and bad legacy
coordinates must not make the canvas open blank or offscreen. If a human manually
moves nodes, the UI may eventually preserve those positions behind an explicit
layout mode, but manual coordinates should not be required for a usable workflow.

A future explicit "save layout" or "lock layout" affordance can preserve human
arrangement, but automatic readable layout remains the default.

## 5. Implementation Slices

### Slice 1: Canvas-First Edit Layout

- Hide the edit-mode runs drawer by default.
- Make the inspector collapsible and selection-driven.
- Keep the toolbar stable.
- Add an empty-canvas call to action for adding the first node.

This immediately improves screen real estate without changing the backend.

### Slice 2: Registry Block Library

- Replace the fixed text palette with a searchable registry-backed library.
- Add rich node cards in the library.
- Support insertion at pointer or viewport center.
- Preserve existing `NodePalette` tests by migrating them to library behavior.

### Slice 3: Rich Node Cards

- Add node summary helpers by node type and field kind.
- Render validation badges and missing required field hints.
- Show meaningful output labels for branch, loop, decision, and dynamic ports.
- Keep cards stable in size so summaries do not resize the canvas erratically.

### Slice 4: Context Menus and Fit Behavior

- Add node/edge/canvas context menus.
- Add duplicate/copy/delete actions.
- Add default auto layout for missing positions and inserted nodes.
- Add fit-view on blueprint load after layout and after template or multi-node
  insertion.

### Slice 5: Inspector and Field Quality

- Improve typed field rendering for `json_schema`, `kv_map`, `branch_port`,
  refs, prompts, and code.
- Scope variable assistant to fields where it helps.
- Add advanced-field grouping once registry metadata supports it or a local
  heuristic is justified.

## 6. Testing

Unit and component tests:

- block library search and category filtering;
- add-node placement from toolbar and canvas context menu;
- inspector opens only for meaningful contexts;
- node card summaries for task, shell, branch, loop, manual trigger, and
  sub-workflow;
- node and edge context actions update the blueprint correctly;
- edit-mode run drawer is hidden by default.

Browser E2E:

- open Workflows edit mode;
- add a manual trigger and task from the library;
- configure required fields;
- connect nodes;
- duplicate/delete a node;
- save and validate;
- capture a feature-specific screenshot for PR evidence.

## 7. Risks and Decisions

- **Risk: a modal-only block library can slow expert users.** Mitigation:
  support right-click insertion and later quick-add from selected nodes or
  edges.
- **Risk: node summaries can become noisy.** Mitigation: summarize only key
  fields, truncate aggressively, and use inspector for detail.
- **Risk: typed field editors can grow into a separate form-builder project.**
  Mitigation: start with registry field kinds already present; improve only the
  high-frequency kinds first.
- **Decision: workflow remains the source of truth.** All restored affordances must
  operate on workflow `Blueprint`, `NodeTypeDef`, and `FieldDef` objects.
- **Decision: auto layout is default.** `position` remains optional editor
  metadata for human-preserved layouts, but authored workflows must not need
  positions.
- **Decision: edit/observe/monitor layouts diverge intentionally.** Edit mode
  maximizes authoring space; observe and monitor can keep run/status panels.

## 8. Acceptance Criteria

- Edit mode uses most of the viewport for the canvas.
- Users can discover nodes through searchable, descriptive registry-backed UI.
- Users can understand common workflow nodes without selecting each one.
- Workflows with omitted node positions render in a readable default layout.
- Workflows with bad persisted positions still open in a readable default
  layout.
- The inspector no longer appears as a low-value permanent panel.
- Runs no longer occupy a permanent edit-mode drawer by default.
- Core authoring actions from old workflow system are available in workflow-native form.
- Existing workflow execution, observe, monitor, and scheduling behavior remain
  unchanged.
