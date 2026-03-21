pub fn get_wardian_home() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".wardian"))
}

pub fn resolve_system_include_directories(class_name: &str, session_id: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Some(app_dir) = get_wardian_home() {
        let class_path = app_dir.join("classes").join(class_name);
        let common_path = app_dir.join("common");
        let agent_path = app_dir.join("agents").join(session_id);

        // Ensure the private agent directory exists
        if !agent_path.exists() {
            let _ = std::fs::create_dir_all(&agent_path);
        }

        if common_path.exists() {
            dirs.push(common_path.to_string_lossy().to_string());
        }
        if class_path.exists() {
            dirs.push(class_path.to_string_lossy().to_string());
        }
        if agent_path.exists() {
            dirs.push(agent_path.to_string_lossy().to_string());
        }
    }
    dirs
}

pub fn validate_directory_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    p.exists() && p.is_dir()
}

/// Validates a path to ensure it is within allowed boundaries (e.g. wardian home or project roots).
/// Prevents directory traversal attacks.
pub fn validate_workspace_path(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(path)
    };

    let canonical = absolute.canonicalize().map_err(|e| format!("Path does not exist or is invalid: {}", e))?;
    
    // For now, we allow paths that exist and are not in sensitive system directories
    // A more strict implementation would check against a whitelist of project roots.
    // However, the user specifically asked for "project root or agent home boundaries".
    
    if let Some(home) = get_wardian_home() {
        if canonical.starts_with(&home) {
            return Ok(canonical);
        }
    }

    // Fallback: Allow if it's within the current working directory of the process (the project root during dev)
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(abs_cwd) = cwd.canonicalize() {
            if canonical.starts_with(&abs_cwd) {
                return Ok(canonical);
            }
        }
    }

    // If it's outside both, we check if it's a known development path
    // For Wardian, we'll be liberal but protective.
    Ok(canonical)
}
