use crate::{state::AppState, workflow::runs};
use serde_json::Value;
use std::collections::HashMap;
use tauri::{AppHandle, State};
use wardian_core::control::WorkflowRunResponse;
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
    let blueprint_path =
        resolve_blueprint_path(&state.blueprint_id).map(|path| path.to_string_lossy().to_string());

    Some(serde_json::json!({
        "run_id": state.run_id,
        "blueprint_id": state.blueprint_id,
        "status": state.status,
        "node_count": state.nodes.len(),
        "failure": state.failure,
        "path": dir.to_string_lossy(),
        "blueprint_path": blueprint_path,
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
    let blueprint_path =
        resolve_blueprint_path(&blueprint_id).map(|path| path.to_string_lossy().to_string());

    Ok(serde_json::json!({
        "state": state,
        "events": events,
        "blueprint": blueprint,
        "blueprint_path": blueprint_path
    }))
}

/// Launch a workflow blueprint run and write durable run artifacts. The default
/// live path routes execution through the running app; CLI mock execution is
/// reserved for workflow-engine tests.
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
) -> Result<WorkflowRunResponse, String> {
    workflow_run_impl(
        state,
        app,
        path,
        provider,
        workspace,
        input,
        bindings,
        assignments,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn workflow_run_from_control(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<WorkflowAssignments>,
) -> Result<WorkflowRunResponse, String> {
    workflow_run_impl(
        state,
        app,
        path,
        provider,
        workspace,
        input,
        bindings,
        assignments,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn workflow_run_impl(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<WorkflowAssignments>,
    control_origin: bool,
) -> Result<WorkflowRunResponse, String> {
    let blueprint = if control_origin {
        parse_control_workflow_blueprint(&path)?
    } else {
        wardian_core::workflow::parse_file(std::path::Path::new(&path))
            .map_err(|e| e.to_string())?
    };
    let report = wardian_core::workflow::validate(&blueprint);
    if !report.is_valid() {
        return Ok(WorkflowRunResponse::validation_failed(
            "live",
            serde_json::to_value(report.diagnostics).map_err(|error| error.to_string())?,
        ));
    }

    let run_id = wardian_core::engine::driver::new_run_id();
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id).ok_or_else(|| {
            format!(
                "invalid workflow run path components for blueprint id `{}` and run id `{run_id}`",
                blueprint.id
            )
        })?;
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
    let run_root_for_run = run_root.clone();
    let run_state = runs::prepare_new_run_with_assignments(
        &blueprint,
        &run_id,
        &run_root,
        &workspace,
        &provider,
        &bindings,
        &assignments,
        input,
    )?;

    tokio::spawn(async move {
        if let Err(error) = runs::drive_started_run_with_catalog_and_assignments(
            Some(app),
            blueprint_for_run,
            run_state,
            run_root_for_run,
            workspace,
            provider,
            bindings,
            assignments,
            agent_catalog,
        )
        .await
        {
            crate::utils::logging::log_debug(&format!("[workflow] run failed: {error}"));
        }
    });

    Ok(WorkflowRunResponse::started(
        "live",
        run_id,
        blueprint.id,
        run_root.to_string_lossy().to_string(),
    ))
}

fn parse_control_workflow_blueprint(path: &str) -> Result<Blueprint, String> {
    let requested = std::path::Path::new(path);
    let requested = std::fs::canonicalize(requested)
        .map_err(|error| format!("workflow path is not readable: {error}"))?;
    let workflows_dir = wardian_core::paths::library_workflows_dir()
        .ok_or_else(|| "no wardian home".to_string())?;
    let workflows_dir = std::fs::canonicalize(&workflows_dir).map_err(|error| {
        format!(
            "workflow library is not readable at {}: {error}",
            workflows_dir.to_string_lossy()
        )
    })?;
    if !requested.starts_with(&workflows_dir) {
        return Err(format!(
            "control workflow_run only accepts files under {}",
            workflows_dir.to_string_lossy()
        ));
    }

    wardian_core::workflow::parse_file(&requested).map_err(|error| error.to_string())
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
    let blueprint = parse_blueprint_for_run(&blueprint_id, &blueprint_path)?;
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
    let blueprint = parse_blueprint_for_run(&blueprint_id, &blueprint_path)?;
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

fn parse_blueprint_for_run(blueprint_id: &str, blueprint_path: &str) -> Result<Blueprint, String> {
    let provided = std::path::Path::new(blueprint_path);
    if !blueprint_path.trim().is_empty() && provided.is_file() {
        return parse_blueprint_file_for_id(provided, blueprint_id);
    }
    let resolved = resolve_blueprint_path(blueprint_id).ok_or_else(|| {
        if blueprint_path.trim().is_empty() {
            format!("could not resolve blueprint path for {blueprint_id}")
        } else {
            format!(
                "could not resolve blueprint path for {blueprint_id}; provided path is not a file: {blueprint_path}"
            )
        }
    })?;
    parse_blueprint_file_for_id(&resolved, blueprint_id)
}

fn parse_blueprint_file_for_id(
    path: &std::path::Path,
    blueprint_id: &str,
) -> Result<Blueprint, String> {
    let blueprint = wardian_core::workflow::parse_file(path).map_err(|e| e.to_string())?;
    if blueprint.id != blueprint_id {
        return Err(format!(
            "blueprint path id mismatch: expected {blueprint_id}, found {}",
            blueprint.id
        ));
    }
    Ok(blueprint)
}

fn resolve_blueprint_path(id: &str) -> Option<std::path::PathBuf> {
    let home = wardian_core::paths::wardian_home()?;
    let dir = home.join("library").join("workflows");
    for entry in walk_md(&dir) {
        if let Ok(bp) = wardian_core::workflow::parse_file(&entry) {
            if bp.id == id {
                return Some(entry);
            }
        }
    }
    None
}

fn resolve_blueprint(id: &str) -> Option<serde_json::Value> {
    let path = resolve_blueprint_path(id)?;
    let blueprint = wardian_core::workflow::parse_file(&path).ok()?;
    serde_json::to_value(blueprint).ok()
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

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(home: &std::path::Path) -> Self {
            let guard = Self {
                _lock: crate::utils::wardian_test_env_lock(),
                previous_home: std::env::var_os("WARDIAN_HOME"),
            };
            std::env::set_var("WARDIAN_HOME", home);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn sample_schedule() -> WorkflowSchedule {
        WorkflowSchedule {
            id: "s1".into(),
            blueprint_id: "heartbeat".into(),
            name: "HB".into(),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: Default::default(),
            assignments: Default::default(),
            schedule: wardian_core::models::ScheduleDefinition {
                schedule_type: "interval".into(),
                interval_minutes: Some(60),
                active: true,
                ..Default::default()
            },
            next_run_epoch_ms: Some(9_999_999_999),
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: None,
            last_run_error: None,
            last_run_epoch_ms: None,
        }
    }

    const WORKFLOW_BLUEPRINT: &str = r#"---
schema: 2
id: wf
name: Workflow
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
edges: []
---

# Workflow
"#;

    const SHELL_WORKFLOW_BLUEPRINT: &str = r#"---
schema: 2
id: shell-wf
name: Shell Workflow
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
  - id: shell
    type: shell
    fields:
      command: echo unsafe
edges:
  - from: trigger
    to: shell
---

# Shell Workflow
"#;

    fn seed_workflow_blueprint(home: &std::path::Path) -> std::path::PathBuf {
        let workflows_dir = home.join("library").join("workflows");
        std::fs::create_dir_all(&workflows_dir).unwrap();
        let path = workflows_dir.join("wf.md");
        std::fs::write(&path, WORKFLOW_BLUEPRINT).unwrap();
        path
    }

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

    #[test]
    fn run_summary_carries_resolved_blueprint_path_separately_from_run_path() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        let blueprint_path = seed_workflow_blueprint(dir.path());
        let run_root = dir
            .path()
            .join("logs")
            .join("workflows")
            .join("wf")
            .join("run-1");
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::AwaitingApproval;
        write_checkpoint(&run_root, &state).unwrap();

        let summary = summarize_run_dir(&run_root).unwrap();

        assert_eq!(
            summary["path"].as_str(),
            Some(run_root.to_string_lossy().as_ref())
        );
        assert_eq!(
            summary["blueprint_path"].as_str(),
            Some(blueprint_path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn parse_blueprint_for_run_falls_back_from_run_dir_to_blueprint_id() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        seed_workflow_blueprint(dir.path());
        let stale_run_dir = dir
            .path()
            .join("logs")
            .join("workflows")
            .join("wf")
            .join("run-1");
        std::fs::create_dir_all(&stale_run_dir).unwrap();

        let blueprint = parse_blueprint_for_run("wf", &stale_run_dir.to_string_lossy()).unwrap();

        assert_eq!(blueprint.id, "wf");
    }

    #[test]
    fn parse_blueprint_for_run_rejects_mismatched_provided_file() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        let other_path = dir.path().join("other.md");
        std::fs::write(
            &other_path,
            WORKFLOW_BLUEPRINT.replace("id: wf", "id: other"),
        )
        .unwrap();

        let error = parse_blueprint_for_run("wf", &other_path.to_string_lossy()).unwrap_err();

        assert!(error.contains("blueprint path id mismatch"));
    }

    #[test]
    fn control_workflow_blueprint_must_live_under_library() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        std::fs::create_dir_all(dir.path().join("library").join("workflows")).unwrap();
        let outside_path = dir.path().join("outside.md");
        std::fs::write(&outside_path, WORKFLOW_BLUEPRINT).unwrap();

        let error = parse_control_workflow_blueprint(&outside_path.to_string_lossy()).unwrap_err();

        assert!(error.contains("only accepts files under"));
    }

    #[test]
    fn control_workflow_blueprint_allows_library_shell_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        let workflows_dir = dir.path().join("library").join("workflows");
        std::fs::create_dir_all(&workflows_dir).unwrap();
        let path = workflows_dir.join("shell.md");
        std::fs::write(&path, SHELL_WORKFLOW_BLUEPRINT).unwrap();

        let blueprint = parse_control_workflow_blueprint(&path.to_string_lossy()).unwrap();

        assert_eq!(blueprint.id, "shell-wf");
        assert!(blueprint.nodes.iter().any(|node| node.r#type == "shell"));
    }

    #[tokio::test]
    async fn schedule_list_reads_persisted_schedules() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());

        wardian_core::schedule::save_schedules(&[sample_schedule()]).unwrap();
        let loaded = schedule_list().await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "s1");
    }

    #[test]
    fn pause_then_resume_round_trips_via_core() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());

        let mut schedule = sample_schedule();
        schedule.is_paused = true;
        schedule.paused_remaining_ms = Some(1234);
        schedule.next_run_epoch_ms = None;
        wardian_core::schedule::save_schedules(&[schedule]).unwrap();

        let loaded = wardian_core::schedule::load_schedules();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].is_paused);
        assert_eq!(loaded[0].paused_remaining_ms, Some(1234));
    }
}
