# Old Workflow System Deletion (sub-project 6d) Design

- **Status:** Implemented
- **Date:** 2026-05-30
- **Part of:** [Workflow rework epic (#425)] — the FINAL parity-track step. Workflow reached parity across 6a (invoker foundation #456), 6b (schedule invoker #457), 6c (monitoring/schedule UI #458), and the 6b test cleanup (#459). 6d removes old workflow system; after it, the epic is complete.

> **Goal:** the codebase is workflow-only — no dead old workflow system code. Two PRs, frontend-first, so the UI never calls a backend command that no longer exists.

## 1. Scope & approach

Delete all old workflow system code (engine, commands, scheduler, the dropped remote/PWA workflow control, CLI old workflow system verbs, old workflow system frontend, and orphaned shared models/types). Split into two independently-green PRs:

- **6d-1 (frontend):** delete the old workflow system frontend cluster + untangle `App.tsx`. After this, nothing in the UI invokes an old workflow system backend command.
- **6d-2 (backend + CLI):** delete the engine, old workflow system Tauri commands, old workflow system scheduler startup, remote/control workflow paths, CLI old workflow system verbs, and orphaned `wardian-core` old workflow system models — now dead with no caller.

**Dependency facts established up front (the deletion is safe because):**
- The workflow surface (`features/workflows/builder/`, `monitor/`, `run/`, `views/WorkflowsView.tsx`, `store/useBuilderStore.ts`, `store/useRunStore.ts`, `store/useSchedulesStore.ts`) imports **none** of the old workflow system cluster.
- The only live coupling from current workflow code into old workflow system is **`App.tsx`** (the old workflow system `useWorkflowStore`, old workflow system Tauri-event listeners, and `RunPayloadModal`).
- `compute_next_run` already lives in `wardian-core::schedule` (old workflow system only imports it), so deleting `workflow_engine` leaves the workflow scheduler intact.
- `ScheduleEditor.tsx` is standalone over `ScheduleDefinition` and is reused by 6c — it **stays**.

## 2. 6d-1 — Frontend deletion + App.tsx untangle (first PR)

### 2.1 Delete the old workflow system cluster (old-workflow-system-only; workflow imports none of these)
```
src/store/useWorkflowStore.ts            (+ .test.ts)
src/views/WorkflowBuilderView.tsx
src/features/workflows/WorkflowSidebar.tsx        (+ .test.tsx)
src/features/workflows/ActiveMonitoring.tsx       (+ .test.tsx)
src/features/workflows/WorkflowNode.tsx
src/features/workflows/RunPayloadModal.tsx        (+ .test.tsx)
src/features/workflows/VariableAssistant.tsx
src/features/workflows/useUpstreamContext.ts
src/features/workflows/workflowLaunch.ts          (+ .test.ts)
src/features/workflows/blockLibrary.ts            (+ .test.ts)
src/components/RenderableInput.tsx                (+ .test.tsx)
src/components/VariablePill.tsx                   (+ .test.tsx)
src/components/SchemaEditor.tsx                    (+ .test.tsx)
```
(`SchemaEditor` is imported only by `WorkflowBuilderView` — confirmed old-workflow-system-only.) The implementation plan re-confirms each file has no surviving current workflow importer via grep before deleting.

### 2.2 Untangle `App.tsx`
Remove, leaving all non-workflow behavior intact:
- the `useWorkflowStore` import + every `useWorkflowStore(...)` subscription (telemetry/progress/status/`fetchWorkflows`/`loadScheduledRuns`/`loadWorkflow`/`saveWorkflow`/`runWorkflowById`/`createScheduledRun`/`setAgents`/`setAgentClasses`);
- the old workflow system **Tauri-event listeners** (workflow telemetry/progress/status/completion) and their handler fns. These events are emitted **only** by `workflow_engine`; the workflow run path writes `events.jsonl` and does not emit them, so removing the listeners drops nothing workflow relies on;
- the `RunPayloadModal` import + render and the `workflowLaunch` import;
- any now-unused props threaded for old workflow system (e.g. `onOpenWorkflowBuilder`/`onOpenWorkflowRunModalInMain` through `SidebarContentPane`) — `npm run build` flags what is truly unused.

> Risk: `App.tsx` also wires non-workflow events; the edit must remove only the workflow-specific listeners/handlers. The full `npm run build` + app smoke is the gate. If a removed old workflow system handler fed a *shared* UI store (e.g. a completion-notification queue), verify the workflow run path already covers that surface before deleting; if not, that's a gap to note — but the old workflow system events are old-workflow-system-emitted, so the expectation is none are shared.

### 2.3 Trim `src/types/workflow.ts`
Keep the workflow-used types (`ScheduleDefinition`, `WorkflowSchedule`, and anything the workflow surface imports); remove old-workflow-system-only types (e.g. old workflow system `WorkflowDefinition`/node/trigger types) once grep confirms no workflow importer.

### 2.4 6d-1 verification
- `npm run lint`, `npm run test`, `npm run build` all green (no dangling imports).
- App smoke: the workflow **Workflows** view (Edit/Observe/Monitor) + left-rail glance render and function; no console errors from missing old workflow system modules.
- No backend change in 6d-1 — old workflow system Tauri commands still exist (now callerless) until 6d-2.

## 3. 6d-2 — Backend + CLI deletion (second PR)

### 3.1 Delete the engine
```
src-tauri/src/workflow_engine/   (whole dir: ~3392 lines — engine, old workflow system start_scheduler,
                                  ScheduledRun-driven scheduling, scheduled_workflows.json IO, migrate.rs)
```

### 3.2 Trim `commands/workflow.rs` + `lib.rs` to workflow only
- Remove the old workflow system command fns (`run_workflow`/`list_workflows`/`show`/`stop`, scheduled-run CRUD, trigger commands) and their `tauri::generate_handler![...]` registrations in `lib.rs`.
- Remove the old workflow system scheduler/trigger **startup** wiring (`start_scheduler`/`start_all_triggers`); keep `workflow::schedule::start_scheduler`.
- Keep all workflow commands (`workflow_run`, `workflow_resume`, `workflow_approve`, `workflow_list_runs`, `workflow_read_run`, `schedule_*`, …) and the `workflow_engine`→`wardian_core::schedule` move (already done in 6b).

### 3.3 Remove the dropped remote/PWA workflow control
- `src-tauri/src/remote/gateway.rs`: remove the workflow routes + handlers (`/remote/api/workflows/run`, `/remote/api/workflows/stop`, the `list_workflows` audited path) and their `workflow_engine` calls — leaving the rest of the gateway intact.
- `src-tauri/src/remote/operations.rs`: remove the `workflow_engine::list_workflows()` operation.
- `src-tauri/src/control.rs`: remove the `workflow_engine` usage (old workflow system control path).

### 3.4 Remove CLI old workflow system verbs
- `crates/wardian-cli/src/args.rs` + `main.rs`: remove the old workflow system `WorkflowCommand` variants `List`/`Show`/`Run`/`Stop` (and their `live::workflow_*` handlers). Keep the workflow verbs (`exec`, `runs`, `run-show`, `replay`, `parse`, `normalize`, `node-types`, `validate`, `gen-schema`, `gen-docs`, `schedule …`).

### 3.5 Remove orphaned wardian-core old workflow system models
- `crates/wardian-core/src/models/workflow.rs`: remove `ScheduledRun`, old workflow system `WorkflowDefinition`, and any old-workflow-system-only telemetry/trigger structs **after** a workspace grep confirms no workflow/CLI use. Keep `ScheduleDefinition` + `WorkflowSchedule`.

### 3.6 6d-2 verification
- `cargo check --workspace`, `cargo test --workspace -- --test-threads=1`, `cargo clippy --workspace -- -D warnings` all green.
- `cargo run -p wardian-cli -- workflow gen-schema --check` + `gen-docs --check` still clean (no node-type change).
- `npm run lint`/`build` green (frontend already workflow-only after 6d-1).

## 4. Data / operational note
`~/.wardian/scheduled_workflows.json` and `~/.wardian/workflows/*.json` are user-local files, never tracked by the repo. After 6d-2 the old workflow system scheduler no longer reads them; the workflow scheduler reads `~/.wardian/library/schedules.json`. Wanted cadences are re-created via the 6c schedule UI (the 5c migration already moved the workflow *definitions* to workflow blueprints). No migration code; no repo data touched.

## 5. Risks / notes
- **`App.tsx` is the delicate edit** (6d-1) — surgical removal of only workflow-specific wiring; the full app build + smoke is the gate.
- **Remote gateway** (6d-2) — excise only the workflow routes; keep terminal/agent/control routes working (the gateway's other tests guard this).
- **wardian-core model removal** (6d-2) — gated by a workspace compile; if any workflow/CLI code unexpectedly references an old workflow system model, keep that type and note it.
- **Ordering** — 6d-1 must merge before 6d-2 so there is no released state where the UI calls a deleted command. (Within 6d-2, the backend becomes dead only after 6d-1 removed the callers.)
- **Privacy** — no user workflow data, names, paths, or webhooks appear in any deleted/edited file or in this spec; all examples are generic. (Tracked separately from the data files under `~/.wardian/`.)
- **Done = epic complete** — after 6d-2 merges, #425 (the workflow rework) is fully delivered and old workflow system is gone.
