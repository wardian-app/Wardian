# Workflow Engine — Unified Workflows View (sub-project 5b) Design

- **Status:** Proposed
- **Date:** 2026-05-29
- **Part of:** [Workflow rework epic (#425)]; follows 1 (library/registry), 2 (engine), 3a (builder), 3b (run view), 4 (CLI), 5a (live executor, #448). Precedes 5c (old workflow system retirement + migration).

> **Structure note:** frontend-only (React/TS). Reuses the 3a builder components and 3b run-view components unchanged; adds one container view + a small UI store, and rewires navigation.

## 1. Problem & goal

The transitional workflow rollout exposed **three** top-level tabs: old workflow system `Workflows` (builder), workflow `Blueprints` (3a edit), workflow `Runs` (3b observe). That's redundant and confusing — the workflow builder and run view are two modes of the *same* blueprint. The architecture spec (§6) called for a single workflow surface with **edit ↔ observe** modes and a **run** affordance.

**Goal of 5b:** collapse the tabs into one **"Workflows"** view that edits a blueprint, runs it, and observes runs — wiring the run lifecycle to 5a's commands. **Remove the old workflow system builder tab.** Reduces the tab count 3 → 1.

**Non-goal / deferred:** a blueprint **library/browser** (listing/organizing all blueprints) belongs to the **Library tab** and is deferred — 5b ships a minimal blueprint open/select affordance only. Also deferred to 5c: old workflow system -> workflow migration and deleting the old workflow system `workflow_engine` / `blockLibrary` code (5b only unwires the old workflow system *tab*).

## 2. Information architecture (Option 3: canvas + mode control + run drawer)

A single `WorkflowsView` is the `workflows` tab. No library landing. Layout:

- **Toolbar (top):** a minimal **blueprint selector** (open an existing blueprint via the `workflow_list_blueprints` command, or **New**), a segmented **[Edit | Observe]** control, and a **Run** button (enabled only when the current blueprint validates clean).
- **Main area:** in **Edit**, the 3a builder (`NodePalette` / `BuilderCanvas` / `NodeConfigForm` + `VariableAssistant` / `DiagnosticsPanel`); in **Observe**, the 3b run view (`RunDag` + `EventTimeline` + `NodeInspector`).
- **Run drawer (collapsible):** lists this blueprint's runs (from `workflow_list_runs`, filtered to the current `blueprint_id`); selecting one observes it. The drawer is the run selector for Observe.

**Navigation change:** `WorkspaceTabs.tsx` + `App.tsx` drop the `workflow-builder` (old workflow system), `workflow-blueprints` (Blueprints), and `workflow-runs` (Runs) entries and add a single `workflows` entry rendering `WorkflowsView`. The old workflow system `WorkflowBuilderView` import/route is removed; the component file is left in place for 5c to delete (no dead-tab, but the file isn't deleted yet to keep 5b's diff focused on navigation).

## 3. State coordination

A small UI store `useWorkflowsView` holds `{ mode: 'edit' | 'observe', blueprintPath: string | null, selectedRunId: string | null }`. It **delegates, not duplicates**:

- **Edit** drives the existing `useBuilderStore` (blueprint, diagnostics, validate, save).
- **Observe** drives the existing `useRunStore` (runs list, selected run state + events, scrub).

Switching Edit ↔ Observe does **not** reset the builder store (edit-in-progress is preserved). Opening a blueprint loads it into the builder store via `workflow_parse`; entering Observe loads that blueprint's runs via `workflow_list_runs` and, if a run is selected, `workflow_read_run`. The two existing stores are unchanged; `useWorkflowsView` only coordinates which is foregrounded and the shared `blueprintPath` / `selectedRunId`.

## 4. Run launch + lifecycle

- **Run button** → a **launch dialog**: a **provider** field defaulting to the agents' default provider — read `default_provider` from `useSettingsStore` and resolve via the same `resolveEffectiveProvider(providerReadiness, defaultProvider)` agent spawn uses (respects provider readiness), overridable per-run — plus an optional **workspace** field. On confirm, call `workflow_run(path, provider, workspace)`, switch to **Observe**, and open the returned run live.
- **Live updates:** while the observed run's status is `Running`, the run store refreshes `workflow_read_run` on a short interval (poll) so the DAG/timeline update as the run progresses; polling stops at a terminal status.
- **Lifecycle controls (Observe):** **Resume** (`workflow_resume`), **Approve / Reject** for an `awaiting_approval` node (`workflow_approve`), **Cancel** (`workflow_cancel`). A run with status `Running` but no active progress (the 5a startup-scan "interrupted" case) is shown with a **Resume** affordance.

## 5. Components & files

```
src/views/WorkflowsView.tsx            # the unified container (toolbar + mode area + run drawer)
src/store/useWorkflowsView.ts          # UI store: mode, blueprintPath, selectedRunId (delegates to builder/run stores)
src/features/workflows/RunLaunchDialog.tsx   # provider (settings default) + workspace -> workflow_run
src/features/workflows/BlueprintSelector.tsx # minimal open-existing (workflow_list_blueprints) / New
src/features/workflows/RunControls.tsx       # Resume/Approve/Reject/Cancel buttons -> 5a commands
src/views/App.tsx                      # route the single `workflows` tab; drop old workflow system/Blueprints/Runs routes
src/layout/titlebar/WorkspaceTabs.tsx  # one `workflows` tab; remove the 3 old entries
```
Reused unchanged: all `src/features/workflows/builder/*` (3a) and `src/features/workflows/run/*` (3b), `useBuilderStore`, `useRunStore`.

## 6. Testing

- **Unit (Vitest/RTL):** `useWorkflowsView` mode transitions + the launch-dialog → `workflow_run` → auto-Observe handoff (mock `invoke`); `RunLaunchDialog` defaults its provider from a mocked `useSettingsStore`; `BlueprintSelector` lists from a mocked `workflow_list_blueprints`.
- **Browser E2E (Playwright, mock invoke):** open the Workflows tab → Edit a blueprint → click Run → dialog (provider prefilled) → confirm → view auto-switches to Observe showing the new run → switch back to Edit and the graph is preserved. Reuses the 3a/3b mock-invoke harness. Assert the three old tabs are gone and one `workflows` tab remains.
- **Screenshots:** the unified view in Edit and in Observe; force-added under `e2e/screenshots/workflows/<timestamp>/` and embedded in the PR.

## 7. Risks / decisions

- **old workflow system tab removed before 5c migration (user-accepted):** existing old workflow system workflows become UI-unreachable in the 5b→5c gap; the old workflow system engine + scheduled runs remain intact (just no builder tab). Mitigation: prioritize 5c; 5b leaves the old workflow system code in place (only the tab is unwired) so re-exposing it is trivial if needed.
- **Library deferred:** with no library browser yet, blueprint discovery in 5b is the minimal selector (`workflow_list_blueprints` + New). The richer library lives in the Library tab later; `WorkflowsView` is designed to accept a blueprint path from elsewhere (so the future Library tab can deep-link into it).
- **Polling for live updates:** a simple interval poll of `workflow_read_run` (no push/file-watch yet) — acceptable for single-user local runs; a watch-based stream is a later enhancement.
