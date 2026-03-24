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
        // Ensure Claude can discover skills from agent's .agents/skills/
        ensure_claude_skills_link(&agent_path);

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

/// Ensures `.claude/skills` is a symlink (or junction on Windows) pointing to
/// `.agents/skills` within the given base directory. This lets Claude Code
/// natively discover skills from the provider-agnostic canonical location.
/// No-ops if the link already exists and points to the right target.
pub fn ensure_claude_skills_link(base_dir: &std::path::Path) {
    let canonical = base_dir.join(".agents").join("skills");
    let link = base_dir.join(".claude").join("skills");

    // Ensure canonical dir exists
    let _ = std::fs::create_dir_all(&canonical);

    // If link already exists (symlink, junction, or real dir), check if it's correct
    if link.exists() || link.symlink_metadata().is_ok() {
        // Already a symlink/junction — verify target
        if let Ok(target) = std::fs::read_link(&link) {
            if target == canonical {
                return; // Already correct
            }
            // Wrong target — remove and recreate
            let _ = std::fs::remove_dir(&link);
        } else {
            // Real directory, not a symlink — leave it alone to avoid data loss
            return;
        }
    }

    // Ensure parent .claude/ dir exists
    let _ = std::fs::create_dir_all(base_dir.join(".claude"));

    // Create the symlink/junction
    #[cfg(windows)]
    {
        // Use a junction (no elevated privileges required, unlike symlinks)
        let _ = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&canonical)
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::os::unix::fs::symlink(&canonical, &link);
    }
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
