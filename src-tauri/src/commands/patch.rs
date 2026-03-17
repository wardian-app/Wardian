use tauri::{AppHandle, Manager};
use std::process::Command;
use crate::manager::log_debug;

#[tauri::command]
pub async fn run_gemini_patch(app: AppHandle) -> Result<String, String> {
    log_debug("[Wardian] Attempting to run Gemini CLI skill patch");
    
    let resource_path = app.path()
        .resource_dir()
        .map_err(|e| format!("Failed to resolve resource directory: {}", e))?
        .join("scripts")
        .join("gemini-patch-skills.cjs");

    if !resource_path.exists() {
        return Err(format!("Patch script not found at {:?}", resource_path));
    }

    log_debug(&format!("[Wardian] Executing node script: {:?}", resource_path));

    let output = Command::new("node")
        .arg(&resource_path)
        .output()
        .map_err(|e| format!("Failed to execute node process: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        log_debug(&format!("[Wardian] Patch successful. Stdout: {}", stdout));
        Ok(stdout)
    } else {
        log_debug(&format!("[Wardian] Patch failed. Stderr: {}", stderr));
        Err(format!("Script exited with status: {}. Stderr: {}", output.status, stderr))
    }
}
