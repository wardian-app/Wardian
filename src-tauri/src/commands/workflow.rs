use crate::{state::AppState, workflow::runs};
use serde_json::Value;
use std::collections::HashMap;
use tauri::{AppHandle, State};
use wardian_core::engine::store::{read_checkpoint, read_events};
use wardian_core::engine::RunStatus;
use wardian_core::models::{InvocationKind, WorkflowAssignments, WorkflowSchedule};
use wardian_core::schedule::{compute_next_run, load_schedules, save_schedules};
use wardian_core::workflow::{self, Blueprint};

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

/// List all workflow runs under `<home>/logs/workflows/<id>/<run_id>/`.
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
            if let Some(summary) = summarize_run_dir(&dir) {
                out.push(summary);
            }
        }
    }

    out.sort_by(|a, b| {
        let a_updated = a.get("updated_at").and_then(Value::as_str).unwrap_or("");
        let b_updated = b.get("updated_at").and_then(Value::as_str).unwrap_or("");
        b_updated.cmp(a_updated).then_with(|| {
            let a_run = a.get("run_id").and_then(Value::as_str).unwrap_or("");
            let b_run = b.get("run_id").and_then(Value::as_str).unwrap_or("");
            b_run.cmp(a_run)
        })
    });

    Ok(out)
}

fn summarize_run_dir(dir: &std::path::Path) -> Option<serde_json::Value> {
    let state = read_checkpoint(dir).ok().flatten()?;
    let events = read_events(dir).unwrap_or_default();
    let started_at = events.first().map(|event| event.ts.clone());
    let updated_at = events.last().map(|event| event.ts.clone());
    let completed_at = match state.status {
        RunStatus::Completed => updated_at.clone(),
        RunStatus::Running | RunStatus::AwaitingApproval | RunStatus::Failed => None,
    };

    Some(serde_json::json!({
        "run_id": state.run_id,
        "blueprint_id": state.blueprint_id,
        "status": state.status,
        "node_count": state.nodes.len(),
        "failure": state.failure,
        "path": dir.to_string_lossy(),
        "started_at": started_at,
        "updated_at": updated_at,
        "completed_at": completed_at,
    }))
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

/// Launch a workflow blueprint run headlessly. Validates, then drives the engine in a
/// background task writing logs/workflows/<id>/<run-id>/. Returns the run id.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn workflow_run(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<WorkflowAssignments>,
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
    let assignments = wardian_core::workflow::assignment::normalize_assignments(
        assignments,
        &bindings,
        InvocationKind::Manual,
    );
    let agent_catalog = runs::agent_catalog_from_state_with_assignments(
        &state,
        &bindings,
        &assignments,
        &workspace,
        &provider,
    )
    .await;
    let blueprint_for_run = blueprint.clone();
    let run_id_for_run = run_id.clone();
    let run_root_for_run = run_root.clone();

    tokio::spawn(async move {
        if let Err(error) = runs::drive_new_run_with_catalog_and_assignments(
            Some(app),
            blueprint_for_run,
            run_id_for_run,
            run_root_for_run,
            workspace,
            provider,
            input,
            bindings,
            assignments,
            agent_catalog,
        )
        .await
        {
            crate::utils::logging::log_debug(&format!("[workflow] run failed: {error}"));
        }
    });

    Ok(serde_json::json!({
        "ok": true,
        "run_id": run_id,
        "blueprint_id": blueprint.id,
        "run_dir": run_root.to_string_lossy(),
    }))
}

/// Resume an interrupted or parked workflow run.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn workflow_resume(
    state: State<'_, AppState>,
    app: AppHandle,
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    provider: Option<String>,
    workspace: Option<String>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<WorkflowAssignments>,
) -> Result<serde_json::Value, String> {
    let blueprint = wardian_core::workflow::parse_file(std::path::Path::new(&blueprint_path))
        .map_err(|e| e.to_string())?;
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let invocation = runs::read_run_invocation(&run_root)?;
    let provider = provider
        .or_else(|| invocation.as_ref().map(|value| value.provider.clone()))
        .unwrap_or_else(|| "codex".to_string());
    let workspace = workspace
        .map(std::path::PathBuf::from)
        .or_else(|| {
            invocation
                .as_ref()
                .map(|value| std::path::PathBuf::from(&value.workspace))
        })
        .unwrap_or_else(|| run_root.clone());
    let bindings = bindings
        .or_else(|| invocation.as_ref().map(|value| value.bindings.clone()))
        .unwrap_or_default();
    let assignments = wardian_core::workflow::assignment::normalize_assignments(
        assignments.or_else(|| invocation.as_ref().map(|value| value.assignments.clone())),
        &bindings,
        InvocationKind::Manual,
    );
    let agent_catalog = runs::agent_catalog_from_state_with_assignments(
        &state,
        &bindings,
        &assignments,
        &workspace,
        &provider,
    )
    .await;
    let owner_id = format!("{}/{}", blueprint.id, run_id);

    tokio::spawn(async move {
        let exec = runs::live_executor_with_catalog_assignments_and_app(
            app,
            workspace,
            provider,
            bindings,
            assignments,
            agent_catalog,
        )
        .with_owner_id(owner_id);
        if let Err(error) = wardian_core::engine::Engine::resume(&blueprint, &run_root, &exec)
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
        {
            crate::utils::logging::log_debug(&format!("[workflow] resume failed: {error}"));
        }
    });

    Ok(serde_json::json!({ "ok": true, "run_id": run_id }))
}

/// Grant or reject an approval gate, resuming the run when granted.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn workflow_approve(
    state: State<'_, AppState>,
    app: AppHandle,
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    node: String,
    granted: bool,
    actor: String,
    note: Option<String>,
    provider: Option<String>,
    workspace: Option<String>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<WorkflowAssignments>,
) -> Result<serde_json::Value, String> {
    let blueprint = wardian_core::workflow::parse_file(std::path::Path::new(&blueprint_path))
        .map_err(|e| e.to_string())?;
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;

    let result = if granted {
        let invocation = runs::read_run_invocation(&run_root)?;
        let provider = provider
            .or_else(|| invocation.as_ref().map(|value| value.provider.clone()))
            .unwrap_or_else(|| "codex".to_string());
        let workspace = workspace
            .map(std::path::PathBuf::from)
            .or_else(|| {
                invocation
                    .as_ref()
                    .map(|value| std::path::PathBuf::from(&value.workspace))
            })
            .unwrap_or_else(|| run_root.clone());
        let bindings = bindings
            .or_else(|| invocation.as_ref().map(|value| value.bindings.clone()))
            .unwrap_or_default();
        let assignments = wardian_core::workflow::assignment::normalize_assignments(
            assignments.or_else(|| invocation.as_ref().map(|value| value.assignments.clone())),
            &bindings,
            InvocationKind::Manual,
        );
        let agent_catalog = runs::agent_catalog_from_state_with_assignments(
            &state,
            &bindings,
            &assignments,
            &workspace,
            &provider,
        )
        .await;
        let exec = runs::live_executor_with_catalog_assignments_and_app(
            app.clone(),
            workspace,
            provider,
            bindings,
            assignments,
            agent_catalog,
        )
        .with_owner_id(format!("{}/{}", blueprint.id, run_id));
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
pub fn workflow_cancel(blueprint_id: String, run_id: String) -> Result<serde_json::Value, String> {
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
    let _ = app.emit("schedules-updated", ());
}

/// Create a workflow schedule. `schedule` is the cadence definition; runtime fields are seeded.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn schedule_create(
    app: AppHandle,
    blueprint_id: String,
    name: String,
    schedule: wardian_core::models::ScheduleDefinition,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<wardian_core::models::WorkflowAssignments>,
) -> Result<WorkflowSchedule, String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    let bindings = bindings.unwrap_or_default();
    let assignments = wardian_core::workflow::assignment::normalize_assignments(
        assignments,
        &bindings,
        InvocationKind::Scheduled,
    );
    let record = WorkflowSchedule {
        id: wardian_core::engine::driver::new_run_id(),
        blueprint_id,
        name,
        provider,
        workspace,
        input: input.unwrap_or_else(|| serde_json::json!({})),
        bindings,
        assignments,
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
pub async fn schedule_list() -> Result<Vec<WorkflowSchedule>, String> {
    Ok(load_schedules())
}

#[tauri::command]
pub async fn schedule_pause(app: AppHandle, id: String) -> Result<(), String> {
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
pub async fn schedule_resume(app: AppHandle, id: String) -> Result<(), String> {
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
pub async fn schedule_remove(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    schedules.retain(|schedule| schedule.id != id);
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

/// Fire ASAP: set next_run to now so the scheduler's next tick launches it live.
#[tauri::command]
pub async fn schedule_run_now(app: AppHandle, id: String) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::engine::event::{Event, EventKind};
    use wardian_core::engine::store::{append_event, write_checkpoint};
    use wardian_core::engine::{RunState, RunStatus};

    #[test]
    fn run_summary_uses_event_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let run_root = dir.path().join("wf").join("run-1");
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::Completed;
        write_checkpoint(&run_root, &state).unwrap();
        append_event(
            &run_root,
            &Event::at(
                0,
                "2026-05-31T12:00:00Z".into(),
                EventKind::RunStarted {
                    blueprint_id: "wf".into(),
                    schema: 2,
                    trigger: serde_json::json!({}),
                },
            ),
        )
        .unwrap();
        append_event(
            &run_root,
            &Event::at(1, "2026-05-31T12:01:00Z".into(), EventKind::RunCompleted),
        )
        .unwrap();

        let summary = summarize_run_dir(&run_root).unwrap();

        assert_eq!(summary["started_at"], "2026-05-31T12:00:00Z");
        assert_eq!(summary["updated_at"], "2026-05-31T12:01:00Z");
        assert_eq!(summary["completed_at"], "2026-05-31T12:01:00Z");
    }
}
