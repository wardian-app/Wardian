use crate::manager::log_debug;
use std::process::Command;
use tauri::{AppHandle, Manager};

#[tauri::command]
pub async fn run_gemini_patch(app: AppHandle) -> Result<String, String> {
    log_debug("[Wardian] Attempting to run Gemini CLI skill patch");

    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Failed to resolve resource directory: {}", e))?;

    // Try a few possible locations based on Tauri resource bundling behavior and dev mode
    let possible_paths = vec![
        resource_dir
            .join("_up_")
            .join("scripts")
            .join("gemini-patch-skills.cjs"), // Bundled via "../scripts/*"
        resource_dir.join("scripts").join("gemini-patch-skills.cjs"), // If it was moved
        std::env::current_dir()
            .unwrap_or_default()
            .join("scripts")
            .join("gemini-patch-skills.cjs"), // Dev mode fallback from project root
        std::env::current_dir()
            .unwrap_or_default()
            .join("../scripts")
            .join("gemini-patch-skills.cjs"), // Dev mode fallback from src-tauri
    ];

    let mut found_path = None;
    for p in possible_paths {
        if p.exists() {
            found_path = Some(p);
            break;
        }
    }

    let resource_path = match found_path {
        Some(p) => p,
        None => {
            return Err(format!(
                "Patch script not found in any expected location. Base resource dir was: {:?}",
                resource_dir
            ))
        }
    };

    log_debug(&format!(
        "[Wardian] Executing node script: {:?}",
        resource_path
    ));

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
        Err(format!(
            "Script exited with status: {}. Stderr: {}",
            output.status, stderr
        ))
    }
}
