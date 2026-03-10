use crate::models::{WorkflowDefinition, WorkflowTelemetryEvent};
use crate::utils::fs::get_wardian_home;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};
use chrono::Utc;

pub fn get_workflows_dir() -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("workflows"))
}

pub fn get_logs_dir(workflow_id: &str) -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("workflow_logs").join(workflow_id))
}

pub fn list_workflows() -> Result<Vec<WorkflowDefinition>, String> {
// ... existing list_workflows ...
    let dir = get_workflows_dir().ok_or("Could not find Wardian home")?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }

    let mut workflows = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
            if let Ok(wf) = serde_json::from_str::<WorkflowDefinition>(&content) {
                workflows.push(wf);
            }
        }
    }
    Ok(workflows)
}

pub fn save_workflow(wf: WorkflowDefinition) -> Result<(), String> {
// ... existing save_workflow ...
    let dir = get_workflows_dir().ok_or("Could not find Wardian home")?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }

    let path = dir.join(format!("{}.json", wf.id));
    let content = serde_json::to_string_pretty(&wf).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn run_workflow(app: AppHandle, wf_id: String) -> Result<(), String> {
    let workflows = list_workflows()?;
    let wf = workflows.into_iter().find(|w| w.id == wf_id)
        .ok_or_else(|| format!("Workflow {} not found", wf_id))?;

    let log_dir = get_logs_dir(&wf_id).ok_or("Could not find log path")?;
    fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let log_path = log_dir.join(format!("{}.json", timestamp));

    // MVP: Simulate execution and log results
    tauri::async_runtime::spawn(async move {
        let mut trace = Vec::new();
        
        for node in &wf.nodes {
            let event = WorkflowTelemetryEvent {
                workflow_id: wf.id.clone(),
                node_id: node.id.clone(),
                status: "processing".to_string(),
                output: None,
                error: None,
            };
            let _ = app.emit("workflow-telemetry", &event);
            trace.push(event);

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let event_done = WorkflowTelemetryEvent {
                workflow_id: wf.id.clone(),
                node_id: node.id.clone(),
                status: "completed".to_string(),
                output: Some(serde_json::json!({"status": "success", "node_type": node.r#type})),
                error: None,
            };
            let _ = app.emit("workflow-telemetry", &event_done);
            trace.push(event_done);
        }

        // Save the execution log
        if let Ok(json) = serde_json::to_string_pretty(&trace) {
            let _ = fs::write(log_path, json);
        }
    });

    Ok(())
}
