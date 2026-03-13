use crate::models::WorkflowDefinition;
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
