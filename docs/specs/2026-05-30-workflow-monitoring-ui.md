# Workflow Engine — Monitoring Sidebar + Schedule-Management UI (sub-project 6c) Design

- **Status:** Implemented
- **Date:** 2026-05-30
- **Part of:** [Workflow rework epic (#425)]; old-system parity track. The UI layer on 6a (invoker foundation, #456) + 6b (schedule invoker, #457). Last parity piece before 6d (old workflow system deletion).

> **Model:** both "run now" and "run on a schedule" are *invokers* of a blueprint (6a). 6c gives schedules a UI — created through the same Run flow — and surfaces workflow activity (active/recent runs + scheduled invokers) at a glance and in a full monitor.

## 1. Goal & scope

Ship the workflow monitoring + schedule-management UI so the old workflow system monitoring sidebar (`WorkflowSidebar`/`ActiveMonitoring`/`ScheduleEditor`) can be retired in 6d. No backend changes — 6c is pure frontend over 6b's commands + events and the existing workflow run-listing commands.

**In scope (6c):**
1. A `useSchedulesStore` (Zustand) over 6b's `schedule_*` commands + the `schedules-updated` event.
2. A full **Monitor mode** in the unified `WorkflowsView` (third tab beside Edit/Observe): schedules table with full inline controls + active/recent runs list.
3. A compact left-rail **glance** pane (replaces the old workflow system `WorkflowSidebar` mount for the `"workflows"` tab).
4. **Schedule creation woven into the Run flow:** a "Run now / Schedule" toggle in `RunLaunchDialog` that reuses `ScheduleEditor` and calls `schedule_create`.

**Out of scope / deferred:** any backend change (6b is complete); remote/PWA workflow control (dropped from the parity track); deleting old workflow system files (6d); CI-test-hardening + browser E2E for the 6b *schedule backend* (the user deferred that pass — 6c still ships its own RTL tests + a 6c browser E2E).

## 2. What already exists (leverage, don't rebuild)

- **6b commands** (Tauri): `schedule_create(blueprint_id, name, schedule, provider?, workspace?, input?, bindings?) -> WorkflowSchedule`, `schedule_list() -> WorkflowSchedule[]`, `schedule_pause(id)`, `schedule_resume(id)`, `schedule_remove(id)`, `schedule_run_now(id)`. Each emits `schedules-updated`.
- **Run listing**: `useRunStore` already exposes `loadRuns()` → `invoke('workflow_list_runs') : RunSummary[]` (all blueprints, each carries `blueprint_id`/`run_id`/`status`/`path`) and `openRun(blueprintId, runId)`. The Monitor's active/recent runs reuse this verbatim.
- **`ScheduleEditor.tsx`** — a presentational form over `ScheduleDefinition` (interval/daily/weekly/monthly/specific_dates/one_time + end conditions), theme-variable styled, no old-workflow-system-engine coupling. **Reused as-is.**
- **`RunLaunchDialog.tsx`** — already collects provider/workspace/input/bindings and calls `workflow_run`. Extended (not rebuilt) with the schedule toggle.
- **Event-subscription precedent**: `useLibraryStore.ts` subscribes to a Tauri event via `listen(...)` from `@tauri-apps/api/event` and reloads — the pattern `useSchedulesStore` follows.
- **`useWorkflowsView`** — the unified view's mode store (`'edit' | 'observe'`); gains `'monitor'`.
- **`SidebarContentPane.tsx`** — renders the left-rail pane per `activeTab`; `"workflows"` currently mounts old workflow system `WorkflowSidebar`.

## 3. Components & files

```
src/types/workflow.ts                                   # + WorkflowSchedule TS type (mirrors 6b Rust DTO)
src/store/useSchedulesStore.ts                          # NEW: schedules state, load, event refresh, actions
src/store/useWorkflowsView.ts                           # mode union + 'monitor'; monitorMode() helper
src/features/workflows/RunLaunchDialog.tsx              # + Run-now / Schedule toggle (reuses ScheduleEditor)
src/features/workflows/ScheduleEditor.tsx               # reused as-is (type import points at shared ScheduleDefinition)
src/features/workflows/monitor/WorkflowMonitor.tsx      # NEW: full Monitor mode (SchedulesTable + ActiveRunsList)
src/features/workflows/monitor/SchedulesTable.tsx       # NEW
src/features/workflows/monitor/ScheduleRow.tsx          # NEW: inline pause/resume/run-now/edit/remove + status badge
src/features/workflows/monitor/ActiveRunsList.tsx       # NEW: running/recent runs -> click opens Observe
src/features/workflows/monitor/WorkflowMonitorGlance.tsx# NEW: left-rail pane (counts + few rows)
src/views/WorkflowsView.tsx                             # render WorkflowMonitor for mode==='monitor'; add tab
src/layout/SidebarContentPane.tsx                       # "workflows" tab -> WorkflowMonitorGlance (was WorkflowSidebar)
```

### 3.1 `useSchedulesStore`
State: `schedules: WorkflowSchedule[]`, `loading`, `error`. Actions:
- `load()` → `invoke('schedule_list')`.
- `subscribe()` → `listen('schedules-updated', () => load())`; returns an unlisten fn. Called once on first mount, cleaned up on unmount.
- `create(args)`, `pause(id)`, `resume(id)`, `remove(id)`, `runNow(id)` → invoke the matching command; each command emits the event, so the store reloads via the subscription (single source of truth = `schedules.json`; no optimistic mirror to drift).
- Errors set `error` (surfaced inline) without throwing the view down.

### 3.2 Monitor mode (`WorkflowMonitor`)
Rendered by `WorkflowsView` when `mode === 'monitor'`. Two regions:
- **`SchedulesTable`** — one `ScheduleRow` per schedule: name, blueprint id, human cadence ("Mon–Fri 09:35"), next-run time, last-run status badge (emerald=completed/idle, cyan=running, amber=paused/awaiting, gray=off, red=failed), and inline actions **pause/resume · run-now · edit · remove**. Edit opens `RunLaunchDialog` pre-filled in schedule mode. Empty state: "No schedules yet — schedule a blueprint from the Run dialog."
- **`ActiveRunsList`** — `RunSummary`s filtered to running/awaiting (active) + a recent tail; each row click → `openRun` + switch to Observe. Polls `loadRuns()` on an interval while Monitor is open (same cadence as Observe's live poll).

### 3.3 Left-rail glance (`WorkflowMonitorGlance`)
Replaces the old workflow system `WorkflowSidebar` for the `"workflows"` tab. Compact: "N active · N scheduled" counts, the active runs (click → Observe), and the next few upcoming schedules (by `next_run_epoch_ms`). A "Manage" / "Open Monitor" affordance switches the main view to Monitor mode. Reads the same two stores; no logic of its own beyond sorting/slicing.

### 3.4 Schedule creation in the Run flow (`RunLaunchDialog`)
Add a segmented **Run now / Schedule** toggle. Default "Run now" = today's behavior (calls `workflow_run`). "Schedule" reveals a name field + the reused `ScheduleEditor`; the primary button becomes **Save schedule** and calls `schedule_create(blueprint_id, name, schedule, provider, workspace, input, bindings)` using the same provider/input/bindings already gathered. Editing an existing schedule reuses this dialog seeded from the `WorkflowSchedule` (on save, `remove` + `create`, or a future `schedule_update` — 6c uses remove+create to avoid a new backend command; flagged in risks).

## 4. Data flow

```
mount → useSchedulesStore.load()  +  subscribe('schedules-updated')
UI action (pause/run-now/create/remove) → invoke schedule_*
   → backend emits 'schedules-updated' → store.load() → re-render
Monitor open → setInterval(loadRuns, ~1500ms) for active-run status
run row click → useRunStore.openRun + useWorkflowsView.observeRun
```

## 5. Error handling
- Command rejection → `useSchedulesStore.error` shown as row/banner text + a Retry; the table stays usable.
- `schedule_list` failure → empty list + error banner + Refresh.
- Event listener registered once and unlistened on unmount (no leak across view remounts).
- A schedule whose `blueprint_id` no longer resolves still lists (with a muted "missing blueprint" note); run-now will surface the backend error.

## 6. Testing
- **RTL — `useSchedulesStore`:** `load` populates from a mocked `schedule_list`; an emitted `schedules-updated` triggers reload; `pause/resume/runNow/remove/create` invoke the correct command with the right args.
- **RTL — `ScheduleRow`/`SchedulesTable`:** renders cadence + next-run + status badge; each inline control calls the matching store action; paused vs active toggles the right icon.
- **RTL — `RunLaunchDialog`:** toggling to Schedule renders `ScheduleEditor` + name; Save calls `schedule_create` with the assembled `ScheduleDefinition` + gathered provider/input/bindings; "Run now" still calls `workflow_run`.
- **RTL — `WorkflowMonitorGlance`:** counts reflect store contents; clicking a run opens Observe.
- **Browser E2E (6c):** Monitor mode → Run dialog → Schedule a blueprint → it appears in the table → pause it (asserts the row state flips). Uses the mock-provider/seeded-home fixture.
- **Frontend PR gate:** capture a Monitor-mode + schedule-form screenshot, embed an HTTPS image in the PR body (e.g. committed to the branch + `raw.githubusercontent.com`, as 6a did) so `check:frontend-screenshot` passes.

## 7. Risks / notes
- **old workflow system entry-point removal:** swapping the `"workflows"` left-rail pane to the workflow glance removes the last old workflow system UI entry. Intended — old workflow system's scheduled workflows were migrated to workflow (5c) and now fire via 6b. old workflow system `WorkflowSidebar.tsx`/`ActiveMonitoring.tsx`/`ScheduleEditor.tsx`-as-old-workflow-system-consumer stay on disk for 6d; only the *mount* changes here (mirrors how 5b removed the old workflow system main tab but left the file).
- **No `schedule_update`:** editing a schedule is remove+create in 6c (id changes). Acceptable for parity; a stable-id update command is a small 6b follow-up if the id churn matters to 6c's monitoring (it doesn't — rows key on id and reload from the event).
- **Two ScheduleDefinition types:** `src/types/workflow.ts` already defines `ScheduleDefinition` for old workflow system; it is structurally identical to 6b's. 6c reuses that type for `ScheduleEditor` and the new `WorkflowSchedule` type, rather than introducing a second definition.
- **Run polling:** Monitor reuses Observe's interval-poll approach (no run event stream yet); acceptable and consistent. A push-based run feed is a later optimization, not 6c.
- **Theme/standards:** all new components use semantic theme variables + the emerald/cyan/amber/gray/red status palette per AGENTS.md; the left pane stays within the existing collapsible-pane shell.
