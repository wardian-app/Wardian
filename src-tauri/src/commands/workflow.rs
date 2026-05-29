use crate::workflow_engine;
use crate::workflow_v2::runs;
use serde_json::Value;
use tauri::AppHandle;
use wardian_core::engine::store::{read_checkpoint, read_events};
use wardian_core::models::{ScheduledRun, WorkflowDefinition};
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
) -> Result<serde_json::Value, String> {
    let blueprint = wardian_core::workflow::parse_file(std::path::Path::new(&blueprint_path))
        .map_err(|e| e.to_string())?;
    let run_root =
        wardian_core::paths::workflow_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let provider = provider.unwrap_or_else(|| "codex".to_string());
    let workspace = workspace
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| run_root.clone());

    tokio::spawn(async move {
        if let Err(error) = runs::drive_resume(blueprint, run_root, workspace, provider).await {
            crate::utils::logging::log_debug(&format!("[workflow-v2] resume failed: {error}"));
        }
    });

    Ok(serde_json::json!({ "ok": true, "run_id": run_id }))
}

/// Grant or reject an approval gate, resuming the run when granted.
#[tauri::command]
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
        let exec = runs::live_executor(workspace, provider);
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
