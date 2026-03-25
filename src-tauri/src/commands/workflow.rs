use crate::models::{WorkflowDefinition, ScheduledRun};
use crate::workflow_engine;
use tauri::AppHandle;
use serde_json::Value;

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
pub async fn run_workflow(app: AppHandle, id: String, payload: Option<Value>) -> Result<(), String> {
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
pub fn load_workflow_library() -> Result<Value, String> {
    Ok(workflow_engine::load_workflow_library())
}

#[tauri::command]
pub fn save_workflow_library(state: Value) -> Result<(), String> {
    workflow_engine::save_workflow_library(&state)
}

#[tauri::command]
pub fn list_scheduled_runs() -> Result<Vec<ScheduledRun>, String> {
    Ok(workflow_engine::load_scheduled_runs())
}

#[tauri::command]
pub fn create_scheduled_run(run: ScheduledRun) -> Result<(), String> {
    let mut runs = workflow_engine::load_scheduled_runs();
    runs.push(run);
    workflow_engine::save_scheduled_runs(&runs)
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
    let mut runs = workflow_engine::load_scheduled_runs();
    if let Some(run) = runs.iter_mut().find(|r| r.id == run_id) {
        run.is_paused = !run.is_paused;
    }
    workflow_engine::save_scheduled_runs(&runs)
}
