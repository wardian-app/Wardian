use crate::workflow_engine;
use crate::workflow_v2::runs;
use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;
use wardian_core::engine::store::{read_checkpoint, read_events};
use wardian_core::models::{ScheduledRun, WorkflowDefinition, WorkflowSchedule};
use wardian_core::schedule::{compute_next_run, load_schedules, save_schedules};
use wardian_core::workflow::{self, Blueprint};

#[tauri::command]
pub async fn list_workflows() -> Result<Vec<WorkflowDefinition>, String> {
    workflow_engine::list_workflows()
}

#[tauri::command]
pub async fn save_workflow(app: AppHandle, workflow: WorkflowDefinition) -> Result<(), String> {
    workflow_engine::save_workflow(app, workflow).await
}

#[tauri::command]
pub async fn delete_workflow(app: AppHandle, id: String) -> Result<(), String> {
    workflow_engine::delete_workflow(app, id).await
}

#[tauri::command]
pub async fn run_workflow(
    app: AppHandle,
    id: String,
    payload: Option<Value>,
) -> Result<(), String> {
    workflow_engine::run_workflow(app, id, payload).await
}

#[tauri::command]
pub async fn stop_all_triggers(app: AppHandle) -> Result<(), String> {
    workflow_engine::stop_all_triggers(app).await;
    Ok(())
}

#[tauri::command]
pub async fn pause_all_triggers(app: AppHandle) -> Result<(), String> {
    workflow_engine::pause_all_triggers(app);
    Ok(())
}

#[tauri::command]
pub async fn resume_all_triggers(app: AppHandle) -> Result<(), String> {
    workflow_engine::resume_all_triggers(app);
    Ok(())
}

#[tauri::command]
pub async fn stop_workflow_triggers(app: AppHandle, workflow_id: String) -> Result<(), String> {
    workflow_engine::stop_workflow_triggers(app, &workflow_id).await;
    Ok(())
}

#[tauri::command]
pub async fn stop_workflow_run(app: AppHandle, run_instance_id: String) -> Result<(), String> {
    workflow_engine::stop_workflow_run(app, &run_instance_id).await;
    Ok(())
}

#[tauri::command]
pub async fn run_scheduled_workflow_now(app: AppHandle, run_id: String) -> Result<(), String> {
    workflow_engine::run_scheduled_workflow_now(app, run_id).await
}

#[tauri::command]
pub fn load_workflow_library() -> Result<Value, String> {
    Ok(workflow_engine::load_workflow_library())
}

#[tauri::command]
pub fn save_workflow_library(state: Value) -> Result<(), String> {
    workflow_engine::save_workflow_library(&state)
}

#[tauri::command]
pub fn list_scheduled_runs() -> Result<Vec<ScheduledRun>, String> {
    workflow_engine::hydrate_scheduled_runs()
}

#[tauri::command]
pub fn create_scheduled_run(run: ScheduledRun) -> Result<(), String> {
    let mut runs = workflow_engine::load_scheduled_runs();
    runs.push(run);
    workflow_engine::save_scheduled_runs(&runs)?;
    let _ = workflow_engine::hydrate_scheduled_runs()?;
    Ok(())
}

#[tauri::command]
pub fn delete_scheduled_run(run_id: String) -> Result<(), String> {
    let runs: Vec<ScheduledRun> = workflow_engine::load_scheduled_runs()
        .into_iter()
        .filter(|r| r.id != run_id)
        .collect();
    workflow_engine::save_scheduled_runs(&runs)
}

#[tauri::command]
pub fn toggle_scheduled_run(run_id: String) -> Result<(), String> {
    let _ = workflow_engine::toggle_scheduled_run_state(&run_id)?;
    Ok(())
}

/// Parse + validate a blueprint `.md` at `path`. Returns the structured graph
/// and any diagnostics (parse errors surface as an Err string).
#[tauri::command]
pub fn workflow_parse(path: String) -> Result<serde_json::Value, String> {
    let blueprint = workflow::parse_file(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    let report = workflow::validate(&blueprint);
    Ok(serde_json::json!({ "blueprint": blueprint, "diagnostics": report.diagnostics }))
}

/// Validate an in-memory blueprint (debounced from the builder on edit).
#[tauri::command]
pub fn workflow_validate(blueprint: Blueprint) -> Result<serde_json::Value, String> {
    let report = workflow::validate(&blueprint);
    Ok(serde_json::json!({ "ok": report.is_valid(), "diagnostics": report.diagnostics }))
}

/// Normalize + serialize + write a blueprint to `path`. Refuses to write while
/// it has validation errors (returns them instead).
#[tauri::command]
pub fn workflow_write(path: String, mut blueprint: Blueprint) -> Result<serde_json::Value, String> {
    workflow::normalize(&mut blueprint);
    let report = workflow::validate(&blueprint);
    if !report.is_valid() {
        return Ok(serde_json::json!({ "written": false, "diagnostics": report.diagnostics }));
    }
    let text = workflow::to_string(&blueprint).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "written": true, "diagnostics": [] }))
}

/// List blueprint `.md` files under `<wardian-home>/library/workflows`.
#[tauri::command]
pub fn workflow_list_blueprints() -> Result<Vec<serde_json::Value>, String> {
    let home = wardian_core::paths::wardian_home().ok_or("no wardian home")?;
    let dir = home.join("library").join("workflows");
    let mut out = Vec::new();
    if dir.exists() {
        for entry in walk_md(&dir) {
            if let Ok(bp) = workflow::parse_file(&entry) {
                out.push(serde_json::json!({ "id": bp.id, "name": bp.name, "path": entry.to_string_lossy() }));
            }
        }
    }
    Ok(out)
}

/// List all v2 runs under `<home>/logs/workflows/<id>/<run_id>/`.
#[tauri::command]
pub fn workflow_list_runs() -> Result<Vec<serde_json::Value>, String> {
    let root = wardian_core::paths::workflow_runs_dir().ok_or("no wardian home")?;
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }

    for bp in std::fs::read_dir(&root)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if !bp.path().is_dir() {
            continue;
        }
        for run in std::fs::read_dir(bp.path())
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let dir = run.path();
            if !dir.is_dir() {
                continue;
            }
            if let Ok(Some(state)) = read_checkpoint(&dir) {
                out.push(serde_json::json!({
                    "run_id": state.run_id,
                    "blueprint_id": state.blueprint_id,
                    "status": state.status,
                    "node_count": state.nodes.len(),
                    "failure": state.failure,
                    "path": dir.to_string_lossy(),
                }));
            }
        }
    }

    Ok(out)
}

/// Read one run: its RunState checkpoint, full event trace, and optional blueprint.
#[tauri::command]
pub fn workflow_read_run(
    blueprint_id: String,
    run_id: String,
) -> Result<serde_json::Value, String> {
    let dir =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let state = read_checkpoint(&dir).map_err(|e| e.to_string())?;
    let events = read_events(&dir).map_err(|e| e.to_string())?;
    let blueprint = resolve_blueprint(&blueprint_id);

    Ok(serde_json::json!({ "state": state, "events": events, "blueprint": blueprint }))
}

/// Launch a v2 blueprint run headlessly. Validates, then drives the engine in a
/// background task writing logs/workflows/<id>/<run-id>/. Returns the run id.
#[tauri::command]
pub async fn workflow_run_v2(
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
) -> Result<serde_json::Value, String> {
    let blueprint = wardian_core::workflow::parse_file(std::path::Path::new(&path))
        .map_err(|e| e.to_string())?;
    let report = wardian_core::workflow::validate(&blueprint);
    if !report.is_valid() {
        return Ok(serde_json::json!({
            "ok": false,
            "diagnostics": report.diagnostics
        }));
    }

    let run_id = wardian_core::engine::driver::new_run_id();
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id).ok_or("no wardian home")?;
    let provider = provider.unwrap_or_else(|| "codex".to_string());
    let workspace = workspace
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| run_root.clone());
    let input = input.unwrap_or_else(|| serde_json::json!({}));
    let bindings = bindings.unwrap_or_default();
    let blueprint_for_run = blueprint.clone();
    let run_id_for_run = run_id.clone();
    let run_root_for_run = run_root.clone();

    tokio::spawn(async move {
        if let Err(error) = runs::drive_new_run(
            blueprint_for_run,
            run_id_for_run,
            run_root_for_run,
            workspace,
            provider,
            input,
            bindings,
        )
        .await
        {
            crate::utils::logging::log_debug(&format!("[workflow-v2] run failed: {error}"));
        }
    });

    Ok(serde_json::json!({
        "ok": true,
        "run_id": run_id,
        "blueprint_id": blueprint.id,
        "run_dir": run_root.to_string_lossy(),
    }))
}

/// Resume an interrupted or parked v2 run.
#[tauri::command]
pub async fn workflow_resume_v2(
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    provider: Option<String>,
    workspace: Option<String>,
    bindings: Option<HashMap<String, String>>,
) -> Result<serde_json::Value, String> {
    let blueprint = wardian_core::workflow::parse_file(std::path::Path::new(&blueprint_path))
        .map_err(|e| e.to_string())?;
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let provider = provider.unwrap_or_else(|| "codex".to_string());
    let workspace = workspace
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| run_root.clone());
    let bindings = bindings.unwrap_or_default();

    tokio::spawn(async move {
        if let Err(error) = runs::drive_resume(blueprint, run_root, workspace, provider, bindings).await {
            crate::utils::logging::log_debug(&format!("[workflow-v2] resume failed: {error}"));
        }
    });

    Ok(serde_json::json!({ "ok": true, "run_id": run_id }))
}

/// Grant or reject an approval gate, resuming the run when granted.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn workflow_approve_v2(
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    node: String,
    granted: bool,
    actor: String,
    note: Option<String>,
    provider: Option<String>,
    workspace: Option<String>,
) -> Result<serde_json::Value, String> {
    let blueprint = wardian_core::workflow::parse_file(std::path::Path::new(&blueprint_path))
        .map_err(|e| e.to_string())?;
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;

    let result = if granted {
        let provider = provider.unwrap_or_else(|| "codex".to_string());
        let workspace = workspace
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| run_root.clone());
        let exec = runs::live_executor(workspace, provider, HashMap::new());
        wardian_core::engine::Engine::grant_approval(
            &blueprint, &run_root, &node, &actor, note, &exec,
        )
        .await
    } else {
        wardian_core::engine::Engine::reject_approval(&blueprint, &run_root, &node, &actor, note)
            .await
    };

    result
        .map(|_| serde_json::json!({ "ok": true }))
        .map_err(|e| e.to_string())
}

/// Record a cancel request for a run. Cooperative cancellation of the live loop
/// is deferred; the marker gives the UI a durable cancellation request.
#[tauri::command]
pub fn workflow_cancel_v2(
    blueprint_id: String,
    run_id: String,
) -> Result<serde_json::Value, String> {
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    std::fs::write(run_root.join("cancel.marker"), "cancelled").map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

fn emit_schedules_updated(app: &AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("v2-schedules-updated", ());
}

/// Create a v2 schedule. `schedule` is the cadence definition; runtime fields are seeded.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn schedule_create_v2(
    app: AppHandle,
    blueprint_id: String,
    name: String,
    schedule: wardian_core::models::ScheduleDefinition,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
) -> Result<WorkflowSchedule, String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    let record = WorkflowSchedule {
        id: wardian_core::engine::driver::new_run_id(),
        blueprint_id,
        name,
        provider,
        workspace,
        input: input.unwrap_or_else(|| serde_json::json!({})),
        bindings: bindings.unwrap_or_default(),
        schedule,
        next_run_epoch_ms: None,
        paused_remaining_ms: None,
        is_paused: false,
        last_run_status: None,
        last_run_error: None,
        last_run_epoch_ms: None,
    };
    let mut record = record;
    record.next_run_epoch_ms = compute_next_run(&record.schedule, now);
    schedules.push(record.clone());
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(record)
}

#[tauri::command]
pub async fn schedule_list_v2() -> Result<Vec<WorkflowSchedule>, String> {
    Ok(load_schedules())
}

#[tauri::command]
pub async fn schedule_pause_v2(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    if let Some(schedule) = schedules.iter_mut().find(|schedule| schedule.id == id) {
        schedule.is_paused = true;
        schedule.paused_remaining_ms = schedule
            .next_run_epoch_ms
            .map(|next_run| next_run.saturating_sub(now));
        schedule.next_run_epoch_ms = None;
    }
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

#[tauri::command]
pub async fn schedule_resume_v2(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    if let Some(schedule) = schedules.iter_mut().find(|schedule| schedule.id == id) {
        schedule.is_paused = false;
        schedule.next_run_epoch_ms = match schedule.paused_remaining_ms.take() {
            Some(remaining) => Some(now.saturating_add(remaining)),
            None => compute_next_run(&schedule.schedule, now),
        };
    }
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

#[tauri::command]
pub async fn schedule_remove_v2(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    schedules.retain(|schedule| schedule.id != id);
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

/// Fire ASAP: set next_run to now so the scheduler's next tick launches it live.
#[tauri::command]
pub async fn schedule_run_now_v2(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    if let Some(schedule) = schedules.iter_mut().find(|schedule| schedule.id == id) {
        schedule.is_paused = false;
        schedule.next_run_epoch_ms = Some(now);
    }
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

fn resolve_blueprint(id: &str) -> Option<serde_json::Value> {
    let home = wardian_core::paths::wardian_home()?;
    let dir = home.join("library").join("workflows");
    for entry in walk_md(&dir) {
        if let Ok(bp) = wardian_core::workflow::parse_file(&entry) {
            if bp.id == id {
                return serde_json::to_value(bp).ok();
            }
        }
    }
    None
}

fn walk_md(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                files.extend(walk_md(&p));
            } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
                files.push(p);
            }
        }
    }
    files
}
