use crate::workflow_engine;
use serde_json::Value;
use tauri::AppHandle;
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
