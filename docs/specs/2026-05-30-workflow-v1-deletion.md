# Workflow Engine v1 Deletion (sub-project 6d) Design

- **Status:** Proposed
- **Date:** 2026-05-30
- **Part of:** [Workflow Engine v2 epic (#425)] — the FINAL parity-track step. v2 reached parity across 6a (invoker foundation #456), 6b (schedule invoker #457), 6c (monitoring/schedule UI #458), and the 6b test cleanup (#459). 6d removes v1; after it, the epic is complete.

> **Goal:** the codebase is v2-only — no dead v1 workflow code. Two PRs, frontend-first, so the UI never calls a backend command that no longer exists.

## 1. Scope & approach

Delete all v1 workflow code (engine, commands, scheduler, the dropped remote/PWA workflow control, CLI v1 verbs, v1 frontend, and orphaned shared models/types). Split into two independently-green PRs:

- **6d-1 (frontend):** delete the v1 frontend cluster + untangle `App.tsx`. After this, nothing in the UI invokes a v1 backend command.
- **6d-2 (backend + CLI):** delete the engine, v1 Tauri commands, v1 scheduler startup, remote/control workflow paths, CLI v1 verbs, and orphaned `wardian-core` v1 models — now dead with no caller.

**Dependency facts established up front (the deletion is safe because):**
- The v2 surface (`features/workflows/builder/`, `monitor/`, `run/`, `views/WorkflowsView.tsx`, `store/useBuilderStore.ts`, `store/useRunStore.ts`, `store/useSchedulesStore.ts`) imports **none** of the v1 cluster.
- The only live coupling from non-v1 code into v1 is **`App.tsx`** (the v1 `useWorkflowStore`, v1 workflow Tauri-event listeners, and `RunPayloadModal`).
- `compute_next_run` already lives in `wardian-core::schedule` (v1 only imports it), so deleting `workflow_engine` leaves the v2 scheduler intact.
- `ScheduleEditor.tsx` is standalone over `ScheduleDefinition` and is reused by 6c — it **stays**.

## 2. 6d-1 — Frontend deletion + App.tsx untangle (first PR)

### 2.1 Delete the v1 cluster (v1-only; v2 imports none of these)
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
(`SchemaEditor` is imported only by `WorkflowBuilderView` — confirmed v1-only.) The implementation plan re-confirms each file has no surviving non-v1 importer via grep before deleting.

### 2.2 Untangle `App.tsx`
Remove, leaving all non-workflow behavior intact:
- the `useWorkflowStore` import + every `useWorkflowStore(...)` subscription (telemetry/progress/status/`fetchWorkflows`/`loadScheduledRuns`/`loadWorkflow`/`saveWorkflow`/`runWorkflowById`/`createScheduledRun`/`setAgents`/`setAgentClasses`);
- the v1 workflow **Tauri-event listeners** (workflow telemetry/progress/status/completion) and their handler fns. These events are emitted **only** by `workflow_engine`; the v2 run path writes `events.jsonl` and does not emit them, so removing the listeners drops nothing v2 relies on;
- the `RunPayloadModal` import + render and the `workflowLaunch` import;
- any now-unused props threaded for v1 (e.g. `onOpenWorkflowBuilder`/`onOpenWorkflowRunModalInMain` through `SidebarContentPane`) — `npm run build` flags what is truly unused.

> Risk: `App.tsx` also wires non-workflow events; the edit must remove only the workflow-specific listeners/handlers. The full `npm run build` + app smoke is the gate. If a removed v1 handler fed a *shared* UI store (e.g. a completion-notification queue), verify the v2 run path already covers that surface before deleting; if not, that's a gap to note — but the v1 workflow events are v1-emitted, so the expectation is none are shared.

### 2.3 Trim `src/types/workflow.ts`
Keep the v2-used types (`ScheduleDefinition`, `WorkflowSchedule`, and anything the v2 surface imports); remove v1-only types (e.g. v1 `WorkflowDefinition`/node/trigger types) once grep confirms no v2 importer.

### 2.4 6d-1 verification
- `npm run lint`, `npm run test`, `npm run build` all green (no dangling imports).
- App smoke: the v2 **Workflows** view (Edit/Observe/Monitor) + left-rail glance render and function; no console errors from missing v1 modules.
- No backend change in 6d-1 — v1 Tauri commands still exist (now callerless) until 6d-2.

## 3. 6d-2 — Backend + CLI deletion (second PR)

### 3.1 Delete the engine
```
src-tauri/src/workflow_engine/   (whole dir: ~3392 lines — engine, v1 start_scheduler,
                                  ScheduledRun-driven scheduling, scheduled_workflows.json IO, migrate.rs)
```

### 3.2 Trim `commands/workflow.rs` + `lib.rs` to v2 only
- Remove the v1 command fns (`run_workflow`/`list_workflows`/`show`/`stop`, scheduled-run CRUD, trigger commands) and their `tauri::generate_handler![...]` registrations in `lib.rs`.
- Remove the v1 scheduler/trigger **startup** wiring (`start_scheduler`/`start_all_triggers`); keep `workflow_v2::schedule::start_v2_scheduler`.
- Keep all v2 commands (`workflow_run_v2`, `workflow_resume_v2`, `workflow_approve_v2`, `workflow_list_runs`, `workflow_read_run`, `schedule_*_v2`, …) and the `workflow_engine`→`wardian_core::schedule` move (already done in 6b).

### 3.3 Remove the dropped remote/PWA workflow control
- `src-tauri/src/remote/gateway.rs`: remove the workflow routes + handlers (`/remote/api/workflows/run`, `/remote/api/workflows/stop`, the `list_workflows` audited path) and their `workflow_engine` calls — leaving the rest of the gateway intact.
- `src-tauri/src/remote/operations.rs`: remove the `workflow_engine::list_workflows()` operation.
- `src-tauri/src/control.rs`: remove the `workflow_engine` usage (v1 workflow control path).

### 3.4 Remove CLI v1 verbs
- `crates/wardian-cli/src/args.rs` + `main.rs`: remove the v1 `WorkflowCommand` variants `List`/`Show`/`Run`/`Stop` (and their `live::workflow_*` handlers). Keep the v2 verbs (`exec`, `runs`, `run-show`, `replay`, `parse`, `normalize`, `node-types`, `validate`, `gen-schema`, `gen-docs`, `schedule …`).

### 3.5 Remove orphaned wardian-core v1 models
- `crates/wardian-core/src/models/workflow.rs`: remove `ScheduledRun`, v1 `WorkflowDefinition`, and any v1-only telemetry/trigger structs **after** a workspace grep confirms no v2/CLI use. Keep `ScheduleDefinition` + `WorkflowSchedule`.

### 3.6 6d-2 verification
- `cargo check --workspace`, `cargo test --workspace -- --test-threads=1`, `cargo clippy --workspace -- -D warnings` all green.
- `cargo run -p wardian-cli -- workflow gen-schema --check` + `gen-docs --check` still clean (no node-type change).
- `npm run lint`/`build` green (frontend already v2-only after 6d-1).

## 4. Data / operational note
`~/.wardian/scheduled_workflows.json` and `~/.wardian/workflows/*.json` are user-local files, never tracked by the repo. After 6d-2 the v1 scheduler no longer reads them; the v2 scheduler reads `~/.wardian/library/schedules.json`. Wanted cadences are re-created via the 6c schedule UI (the 5c migration already moved the workflow *definitions* to v2 blueprints). No migration code; no repo data touched.

## 5. Risks / notes
- **`App.tsx` is the delicate edit** (6d-1) — surgical removal of only workflow-specific wiring; the full app build + smoke is the gate.
- **Remote gateway** (6d-2) — excise only the workflow routes; keep terminal/agent/control routes working (the gateway's other tests guard this).
- **wardian-core model removal** (6d-2) — gated by a workspace compile; if any v2/CLI code unexpectedly references a "v1" model, keep that type and note it.
- **Ordering** — 6d-1 must merge before 6d-2 so there is no released state where the UI calls a deleted command. (Within 6d-2, the backend becomes dead only after 6d-1 removed the callers.)
- **Privacy** — no user workflow data, names, paths, or webhooks appear in any deleted/edited file or in this spec; all examples are generic. (Tracked separately from the data files under `~/.wardian/`.)
- **Done = epic complete** — after 6d-2 merges, #425 (the v2 rework) is fully delivered and v1 is gone.
