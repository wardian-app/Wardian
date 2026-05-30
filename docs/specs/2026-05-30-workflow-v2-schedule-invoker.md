# Workflow Engine v2 — Schedule Invoker (sub-project 6b) Design

- **Status:** Proposed
- **Date:** 2026-05-30
- **Part of:** [Workflow Engine v2 epic (#425)]; v1-parity track. Builds on 6a (invoker foundation, #456). Followed by 6c (monitoring sidebar — schedule-management UI) and 6d (v1 deletion).

> **Model (from 6a):** a *run* is an instance of a blueprint; an **invoker** supplies the invocation context `{ blueprint, input, bindings }`. A **schedule** is a *persisted invoker* that fires on a time cadence: it stores that context plus a `ScheduleDefinition`. The schedule is the only new entity; firing reuses 6a's run path unchanged. This is the first concrete invoker built on the 6a contract (manual being the zeroth).

## 1. Goal & scope

Give v2 the ability to run blueprints on a schedule — the parity gap that made the 5c migration lossy (every Trident + heartbeat workflow was scheduled). A persisted schedule fires real v2 runs via 6a's `workflow_run_v2` path at the right times, survives app restart, and supports the same cadences v1 had.

**In scope (6b):**
1. A `WorkflowSchedule` entity (wardian-core) = invocation context + `ScheduleDefinition` + runtime fields.
2. A tick-loop scheduler (src-tauri) that fires due schedules through the 6a run path with the live executor.
3. Persistence at `<home>/library/schedules.json`, surviving restart.
4. Tauri commands + CLI verbs for schedule CRUD + pause/resume/run-now. (Schedule-management **UI** is deferred to 6c.)

**Out of scope / deferred:** schedule-management UI (6c); remote/PWA workflow control (dropped from the parity track); skip-if-already-running overlap protection (noted as a future guard — 6b fires regardless, matching v1); file-watch / webhook invokers (later, same rails).

## 2. Constraints (locked)

- **Internal epoch scheduler, NOT cron.** Use Wardian's epoch-ms model (`ScheduleDefinition` + a `compute_next_run(&schedule, now_ms) -> Option<u64>` + a periodic tick), for cross-OS consistency. v1's scheduler is the behavior reference, not the implementation.
- **Skip-missed catch-up.** `compute_next_run(now_ms)` always projects the next *future* fire time; missed slots (app was off) are never backfilled.
- **Schedule cadences:** `interval` / `daily` / `weekly` / `monthly` / `specific_dates` / `one_time`, with end conditions `never` / `on_date` / `after_occurrences` — i.e. the existing `ScheduleDefinition` shape, unchanged.

## 3. What already exists (leverage, don't rebuild)

- `wardian_core::models::ScheduleDefinition` — the full cadence/end-condition shape (interval/daily/weekly/monthly/specific_dates/one_time, `time_of_day`, `days_of_week`, `repeat_every`, `days_of_month`, `specific_dates`, `run_at`, `end_condition`, `end_date`, `max_occurrences`, `occurrence_count`, `active`). **Reused verbatim.**
- `compute_next_run(&ScheduleDefinition, now_ms)` — currently a private fn in `src-tauri/src/workflow_engine/mod.rs` (handles every schedule_type, with weekly epoch alignment). **Lifted into wardian-core** and shared; v1 re-imports it (behavior identical, its existing tests move with it).
- 6a's run path: `commands::workflow::workflow_run_v2(path, provider?, workspace?, input?, bindings?)` → resolves the blueprint, builds `run_id`/`run_root`, spawns `runs::drive_new_run(blueprint, run_id, run_root, workspace, default_provider, input, bindings)` with the `LiveStepExecutor`. **The scheduler fires through this same path** (no duplicate execution logic).
- v1's tick-loop structure in `workflow_engine::start_scheduler` — the reference for: 5s `tokio::time::sleep` loop held in `state.scheduler_handle` (abortable/restartable), a paused gate, the "load fresh + merge runtime fields before save" anti-clobber dance, end-condition expiry, occurrence increment, one_time/specific_dates auto-delete. v2 reimplements this cleanly against `WorkflowSchedule`.

## 4. Changes

### 4.1 `WorkflowSchedule` entity (`wardian-core`)
New struct in `crates/wardian-core/src/models/workflow.rs`:
```rust
pub struct WorkflowSchedule {
    pub id: String,                              // schedule id (uuid)
    pub blueprint_id: String,                    // resolves to library/workflows/<id>.md
    pub name: String,                            // display name
    #[serde(default)] pub provider: Option<String>,   // override; else settings default_provider
    #[serde(default)] pub workspace: Option<String>,  // override; else run_root
    #[serde(default)] pub input: serde_json::Value,    // entry input params (6a)
    #[serde(default)] pub bindings: std::collections::HashMap<String, String>, // role/class -> target (6a)
    pub schedule: ScheduleDefinition,            // reused cadence/end-condition shape
    // --- runtime (managed by the scheduler) ---
    #[serde(default)] pub next_run_epoch_ms: Option<u64>,
    #[serde(default)] pub paused_remaining_ms: Option<u64>,
    #[serde(default)] pub is_paused: bool,
    #[serde(default)] pub last_run_status: Option<String>,   // "completed" | "failed" | "running"
    #[serde(default)] pub last_run_error: Option<String>,
    #[serde(default)] pub last_run_epoch_ms: Option<u64>,
}
```
This is v1's `ScheduledRun` re-cut for the invoker model: `workflow_id`→`blueprint_id`, `role_mappings`→`bindings`, plus 6a's `input`/`provider`/`workspace`. v1's `ScheduledRun` is left untouched (v1 still uses it; both deleted together in 6d).

### 4.2 `wardian-core::schedule` module (new)
`crates/wardian-core/src/schedule/mod.rs` — pure, Tauri-free, fully unit-tested:
- `compute_next_run(&ScheduleDefinition, now_ms) -> Option<u64>` — **moved** from `workflow_engine` (its weekly-alignment + per-type tests move too). `workflow_engine` swaps its private fn for `wardian_core::schedule::compute_next_run` (one import; behavior identical).
- `schedules_path() -> PathBuf` — `<home>/library/schedules.json` (via the existing wardian-core paths/home helper).
- `load_schedules() -> Vec<WorkflowSchedule>` — tolerant: missing/malformed file → empty (logged), never panics.
- `save_schedules(&[WorkflowSchedule]) -> io::Result<()>` — **atomic** (write temp + rename). File shape: `{ "schema": 1, "schedules": [ … ] }`.
- `is_expired(&WorkflowSchedule, now) -> bool` and `advance_after_fire(&mut WorkflowSchedule, now_ms)` — pure end-condition / occurrence / next_run transition helpers (so the tick loop is thin and the logic is unit-tested without a running app).

### 4.3 v2 scheduler tick loop (`src-tauri`)
`src-tauri/src/workflow_v2/schedule.rs` (new): `start_v2_scheduler(app: AppHandle)`.
- Cancels any existing handle, spawns a 5s `tokio` loop stored in a new `state.v2_scheduler_handle` (abortable/restartable, mirrors `scheduler_handle`).
- Each tick (skipped while a `v2_schedules_paused` flag is set): `load_schedules()`, `now_ms = Utc::now().timestamp_millis()`. For each schedule that is `active` && !`is_paused`:
  1. If end condition met (`is_expired`) → mark for removal.
  2. If `next_run_epoch_ms` is `None` → set from `paused_remaining_ms` (resume) or `compute_next_run(now_ms)`.
  3. If `now_ms >= next_run_epoch_ms` → **fire**: resolve `library/workflows/<blueprint_id>.md`, default provider (settings) / workspace (run_root), and invoke the **6a path** (`workflow_run_v2`-equivalent → spawn `drive_new_run` with `input` + `bindings`). Then `advance_after_fire` (occurrence++, `next_run = compute_next_run(now_ms)`, one_time / exhausted specific_dates → remove). Record `last_run_*`.
- After the pass: re-load fresh, merge only runtime fields onto the fresh structural list (so concurrent CLI add/remove/pause edits aren't clobbered), `save_schedules`, `app.emit("v2-schedules-updated", ())`.
- Started on app launch alongside the existing startup wiring.

A fire failure is logged into `last_run_error` and does **not** stop the loop; `next_run` still advances.

### 4.4 Tauri commands (`src-tauri/src/commands/workflow.rs`)
- `schedule_create_v2(blueprint_id, name, provider?, workspace?, input?, bindings?, schedule)` → persists a new `WorkflowSchedule` (id assigned, `next_run` computed), returns it.
- `schedule_list_v2()` → `Vec<WorkflowSchedule>`.
- `schedule_pause_v2(id)` / `schedule_resume_v2(id)` → toggle `is_paused`; pause stores `paused_remaining_ms = next_run - now` and clears `next_run`; resume restores it.
- `schedule_remove_v2(id)`.
- `schedule_run_now_v2(id)` → sets `next_run_epoch_ms = now` so the next tick fires it with the live executor (no separate exec path; the loop is the single live driver).
All persist via `wardian_core::schedule` and emit `v2-schedules-updated`.

### 4.5 CLI verbs (`wardian-cli`)
`wardian workflow schedule <sub>` (parity with 6a's `exec`):
- `add --blueprint <id> --name <n> [--every <mins> | --daily HH:MM | --weekly <days> HH:MM | --at <iso>] [--input <json>] [--bind name=target] [--provider <p>]`
- `list` (table / `--json`), `pause <id>`, `resume <id>`, `remove <id>`, `run-now <id>`.
CRUD verbs edit `schedules.json` directly via `wardian_core::schedule` (no running app needed). **`run-now` just sets `next_run = now`** — the running app's tick fires it live; if the app is off, nothing fires (acceptable). The CLI has no live executor, so it never drives a real run itself.

## 5. Components & files (indicative)
```
crates/wardian-core/src/models/workflow.rs   # + WorkflowSchedule struct
crates/wardian-core/src/schedule/mod.rs       # NEW: compute_next_run (moved) + load/save + pure helpers
src-tauri/src/workflow_engine/mod.rs          # swap private compute_next_run -> wardian_core::schedule::compute_next_run
src-tauri/src/workflow_v2/schedule.rs         # NEW: start_v2_scheduler tick loop
src-tauri/src/workflow_v2/mod.rs              # export schedule module
src-tauri/src/commands/workflow.rs            # + schedule_*_v2 commands
src-tauri/src/state.rs                        # + v2_scheduler_handle + v2_schedules_paused
src-tauri/src/lib.rs                          # start_v2_scheduler on launch; register commands
crates/wardian-cli/src/...                    # workflow schedule add/list/pause/resume/remove/run-now
```

## 6. Testing
- **wardian-core unit:** `compute_next_run` per `schedule_type` (interval/daily/weekly/monthly/specific_dates/one_time) including weekly epoch alignment (move v1's tests + extend); skip-missed projection (an overdue schedule projects forward, never to a past slot); `is_expired`/`advance_after_fire` end-condition + occurrence transitions; `save`→`load` round-trip including atomic-write behavior.
- **src-tauri integration (mock provider, reuse 5a harness):** a due schedule fires → a run dir / `drive_new_run` is produced; a paused schedule does not fire; a `one_time` schedule is removed after firing; the reload-merge keeps a concurrently-added schedule.
- **CLI:** `add → list → pause → resume → remove` round-trip against a temp `WARDIAN_HOME`; `run-now` sets `next_run = now`.
- **Drift guards:** none new (no registry/node-type change). `cargo clippy --workspace -- -D warnings`, `cargo test`, `npm run lint`/`build` stay green.

## 7. Risks / notes
- **Lifting `compute_next_run` touches v1.** Mitigated: it's a pure fn with existing tests that move with it; v1 swaps one private call for the shared import — no behavior change. Verified green by v1's own scheduler tests post-move.
- **Two schedulers run concurrently** (v1's `start_scheduler` over `scheduled-runs` and v2's `start_v2_scheduler` over `schedules.json`) until 6d. They read/write **different files** and fire **different engines**, so they don't interfere. v1 keeps serving any not-yet-migrated scheduled runs; v2 serves `WorkflowSchedule`s.
- **Overlap:** 6b fires a due schedule even if its previous run is still active (v1 parity). Skip-if-already-running is a future guard (would track a live run handle per schedule), intentionally deferred.
- **Time zone:** `daily`/`weekly`/`monthly` honor local time (`chrono::Local`), matching v1, so cadences land at the wall-clock time the user authored.
- **No UI in 6b.** Schedules are created/managed via CLI or programmatic Tauri calls; 6c adds the management surface in the monitoring sidebar and consumes `v2-schedules-updated`.
