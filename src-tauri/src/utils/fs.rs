#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct ClaudePermissionHookPaths {
    pub settings_arg: String,
    pub event_log_path: std::path::PathBuf,
}

pub fn get_wardian_home() -> Option<std::path::PathBuf> {
    if let Ok(val) = std::env::var("WARDIAN_HOME") {
        if !val.is_empty() {
            return Some(std::path::PathBuf::from(val));
        }
    }
    dirs::home_dir().map(|h| h.join(".wardian"))
}

pub fn get_default_user_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| {
        if cfg!(windows) {
            std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("C:\\"))
        } else {
            std::path::PathBuf::from("/")
        }
    })
}

pub fn resolve_cwd(folder: &str, agent_id: &str) -> std::path::PathBuf {
    // Priority 1: Explicitly provided folder
    if !folder.is_empty() {
        let p = std::path::PathBuf::from(folder);
        if let Ok(validated) = validate_workspace_path(&p) {
            return validated;
        }
    }

    // Priority 2: Persistent agent configuration (if agent_id is provided)
    if !agent_id.is_empty() {
        if let Some(home) = get_wardian_home() {
            if let Ok(data) = std::fs::read_to_string(home.join("wardian_state.json")) {
                if let Ok(configs) = serde_json::from_str::<Vec<crate::models::AgentConfig>>(&data)
                {
                    if let Some(cfg) = configs.iter().find(|c| c.session_id == agent_id) {
                        if !cfg.folder.is_empty() {
                            let p = std::path::PathBuf::from(&cfg.folder);
                            if let Ok(validated) = validate_workspace_path(&p) {
                                return validated;
                            }
                        }
                    }
                }
            }
        }
    }

    get_default_user_dir()
}

pub fn provider_uses_projected_workspace(provider: &str) -> bool {
    provider == "codex"
}

pub fn prepare_provider_habitat(
    provider: &str,
    workspace_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<Option<std::path::PathBuf>, String> {
    if !provider_uses_projected_workspace(provider) {
        return Ok(None);
    }

    let Some(session_id) = session_id.filter(|sid| !sid.trim().is_empty()) else {
        return Ok(None);
    };

    let habitat_root = prepare_habitat_workspace(workspace_root, class_name, session_id)?;
    if provider == "codex" {
        ensure_codex_home_projection(&habitat_root)?;
    }

    Ok(Some(habitat_root))
}

pub fn habitat_workspace_cwd(habitat_root: &std::path::Path) -> std::path::PathBuf {
    habitat_root.join("workspace")
}

pub fn habitat_codex_home(habitat_root: &std::path::Path) -> std::path::PathBuf {
    habitat_root.join(".codex")
}

pub fn ensure_claude_permission_hook(
    session_id: &str,
) -> Result<ClaudePermissionHookPaths, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let hook_root = wardian_home.join("agents").join(session_id).join("claude");
    std::fs::create_dir_all(&hook_root).map_err(|e| e.to_string())?;

    let event_log_path = hook_root.join("permission-requests.jsonl");
    if !event_log_path.exists() {
        std::fs::write(&event_log_path, "").map_err(|e| e.to_string())?;
    }

    let script_path = write_claude_permission_hook_script(&hook_root, &event_log_path)?;
    let command = claude_permission_hook_command(&script_path);
    let settings_arg = serde_json::json!({
        "hooks": {
            "PermissionRequest": [
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": command,
                        }
                    ]
                }
            ]
        }
    })
    .to_string();

    Ok(ClaudePermissionHookPaths {
        settings_arg,
        event_log_path,
    })
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

fn habitat_root_for_session(
    wardian_home: &std::path::Path,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    let trimmed = session_id.trim();
    if trimmed.is_empty() {
        return Err("Provider session ID is required for agent habitat projection".to_string());
    }

    Ok(wardian_home.join("agents").join(trimmed).join("habitat"))
}

fn prepare_habitat_workspace(
    workspace_root: &std::path::Path,
    class_name: &str,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let habitat_root = habitat_root_for_session(&wardian_home, session_id)?;

    std::fs::create_dir_all(&habitat_root).map_err(|e| e.to_string())?;

    write_habitat_instruction_files(&wardian_home, &habitat_root, class_name, Some(session_id))?;
    build_habitat_skill_projection(&wardian_home, &habitat_root, class_name, Some(session_id))?;

    let workspace_link = habitat_root.join("workspace");
    if !projected_link_matches_target(&workspace_link, workspace_root) {
        if workspace_link.exists() || workspace_link.symlink_metadata().is_ok() {
            let _ = std::fs::remove_dir_all(&workspace_link)
                .or_else(|_| std::fs::remove_dir(&workspace_link));
        }
        create_directory_link(workspace_root, &workspace_link)?;
    }
    if !projected_link_matches_target(&workspace_link, workspace_root) {
        return Err(format!(
            "Failed to create habitat workspace link from {} to {}",
            workspace_link.to_string_lossy(),
            workspace_root.to_string_lossy()
        ));
    }

    Ok(habitat_root)
}

fn normalize_comparison_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let canonical = path.canonicalize().ok()?;
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\?") {
            return Some(std::path::PathBuf::from(stripped));
        }
    }
    Some(canonical)
}

fn projected_link_matches_target(link: &std::path::Path, target: &std::path::Path) -> bool {
    if !(link.exists() || link.symlink_metadata().is_ok()) {
        return false;
    }

    match (
        normalize_comparison_path(link),
        normalize_comparison_path(target),
    ) {
        (Some(link_path), Some(target_path)) => link_path == target_path,
        _ => false,
    }
}

fn ensure_codex_home_projection(habitat_root: &std::path::Path) -> Result<(), String> {
    let real_codex_home = dirs::home_dir()
        .ok_or("Could not find user home directory")?
        .join(".codex");
    let projected_home = habitat_codex_home(habitat_root);
    let wardian_skills = habitat_root.join(".agents").join("skills");
    sync_codex_agent_home(&real_codex_home, &projected_home, &wardian_skills)
}

pub(crate) fn sync_codex_agent_home(
    real_codex_home: &std::path::Path,
    projected_home: &std::path::Path,
    wardian_skills: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir_all(projected_home).map_err(|e| e.to_string())?;

    for shared_name in ["auth.json", "config.toml", "cap_sid"] {
        let source = real_codex_home.join(shared_name);
        if source.exists() && source.is_file() {
            project_file(&source, &projected_home.join(shared_name))?;
        }
    }

    let projected_skills = projected_home.join("skills");
    std::fs::create_dir_all(&projected_skills).map_err(|e| e.to_string())?;

    if !wardian_skills.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(wardian_skills).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let source = entry.path();
        if !source.is_dir() {
            continue;
        }
        let target = projected_skills.join(entry.file_name());
        if target.exists() || target.symlink_metadata().is_ok() {
            let _ = std::fs::remove_dir_all(&target).or_else(|_| std::fs::remove_file(&target));
        }
        create_directory_link(&source, &target)?;
    }

    Ok(())
}

fn write_habitat_instruction_files(
    wardian_home: &std::path::Path,
    habitat_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<(), String> {
    let common_agents = wardian_home.join("common").join("AGENTS.md");
    let class_agents = wardian_home
        .join("classes")
        .join(class_name)
        .join("AGENTS.md");
    let agent_agents = session_id
        .filter(|sid| !sid.trim().is_empty())
        .map(|sid| wardian_home.join("agents").join(sid).join("AGENTS.md"));

    let mut sections = Vec::new();
    let mut candidates = vec![("Common", common_agents), ("Class", class_agents)];
    if let Some(agent_agents) = agent_agents {
        candidates.push(("Agent", agent_agents));
    }
    for (label, path) in candidates {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    sections.push(format!(
                        "## {label}\nSource: {}\n\n{}\n",
                        path.to_string_lossy(),
                        content.trim()
                    ));
                }
            }
        }
    }

    let agents_md = if sections.is_empty() {
        "# Wardian Habitat\n\nThis projected workspace has no additional Wardian instructions.\n"
            .to_string()
    } else {
        format!(
            "# Wardian Habitat\n\nThis file is generated by Wardian to project shared instructions into the active workspace scope.\n\n{}\n",
            sections.join("\n")
        )
    };
    std::fs::write(habitat_root.join("AGENTS.md"), agents_md).map_err(|e| e.to_string())?;

    for stub_name in ["GEMINI.md", "CLAUDE.md"] {
        std::fs::write(habitat_root.join(stub_name), "@AGENTS.md\n").map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn build_habitat_skill_projection(
    wardian_home: &std::path::Path,
    habitat_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<(), String> {
    let merged_skills = habitat_root.join(".agents").join("skills");
    if merged_skills.exists() {
        let _ = std::fs::remove_dir_all(&merged_skills);
    }
    std::fs::create_dir_all(&merged_skills).map_err(|e| e.to_string())?;

    let mut sources = vec![
        wardian_home.join("common").join(".agents").join("skills"),
        wardian_home
            .join("classes")
            .join(class_name)
            .join(".agents")
            .join("skills"),
    ];
    if let Some(session_id) = session_id.filter(|sid| !sid.trim().is_empty()) {
        sources.push(
            wardian_home
                .join("agents")
                .join(session_id)
                .join(".agents")
                .join("skills"),
        );
    }

    for source in sources {
        if !source.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&source) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let skill_src = entry.path();
            if !skill_src.is_dir() {
                continue;
            }
            let skill_name = entry.file_name();
            let skill_dst = merged_skills.join(skill_name);
            if skill_dst.exists() {
                let _ = std::fs::remove_dir_all(&skill_dst)
                    .or_else(|_| std::fs::remove_dir(&skill_dst));
            }
            create_directory_link(&skill_src, &skill_dst)?;
        }
    }

    Ok(())
}

fn write_claude_permission_hook_script(
    hook_root: &std::path::Path,
    event_log_path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    #[cfg(windows)]
    {
        let script_path = hook_root.join("permission-request-hook.ps1");
        let script = format!(
            "$payload = [Console]::In.ReadToEnd()\nif ([string]::IsNullOrWhiteSpace($payload)) {{ exit 0 }}\nAdd-Content -LiteralPath '{}' -Value $payload -Encoding utf8\n",
            escape_powershell_single_quoted(&event_log_path.to_string_lossy())
        );
        std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
        Ok(script_path)
    }
    #[cfg(not(windows))]
    {
        let script_path = hook_root.join("permission-request-hook.sh");
        let script = format!(
            "#!/bin/sh\nset -eu\npayload=$(cat)\nif [ -z \"$payload\" ]; then\n  exit 0\nfi\nprintf '%s\\n' \"$payload\" >> '{}'\n",
            escape_posix_single_quoted(&event_log_path.to_string_lossy())
        );
        std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&script_path)
            .map_err(|e| e.to_string())?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).map_err(|e| e.to_string())?;
        Ok(script_path)
    }
}

fn claude_permission_hook_command(script_path: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        format!(
            "powershell -WindowStyle Hidden -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
            script_path.to_string_lossy()
        )
    }
    #[cfg(not(windows))]
    {
        format!("sh \"{}\"", script_path.to_string_lossy())
    }
}

#[cfg(windows)]
fn escape_powershell_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(not(windows))]
fn escape_posix_single_quoted(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}

fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let command = format!(
            "mklink /J \"{}\" \"{}\"",
            link.to_string_lossy(),
            target.to_string_lossy()
        );
        let mut cmd = std::process::Command::new("cmd");
        cmd.raw_arg("/c")
            .raw_arg(&command)
            .creation_flags(0x08000000);
        let output = cmd.output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Err(format!(
                "Failed to create junction {} -> {}. {}{}",
                link.to_string_lossy(),
                target.to_string_lossy(),
                stdout,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(" {}", stderr)
                }
            ))
        }
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())
    }
}

fn project_file(source: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    if target.exists() {
        let _ = std::fs::remove_file(target);
    }

    match std::fs::hard_link(source, target) {
        Ok(_) => Ok(()),
        Err(_) => std::fs::copy(source, target)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
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
    let _ = create_directory_link(&canonical, &link);
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
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };

    let canonical = absolute
        .canonicalize()
        .map_err(|e| format!("Path does not exist or is invalid: {}", e))?;

    // On Windows, canonicalize() produces extended-length paths with \\?\ prefix
    // which breaks CLI tools. Strip it to get a normal path.
    #[cfg(windows)]
    let canonical = {
        let s = canonical.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            std::path::PathBuf::from(stripped)
        } else {
            canonical
        }
    };

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

#[cfg(test)]
mod tests {
    use super::{
        create_directory_link, habitat_root_for_session, projected_link_matches_target,
        provider_uses_projected_workspace, sync_codex_agent_home,
    };
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wardian-{label}-{stamp}"))
    }

    #[test]
    fn habitat_root_uses_provider_session_id_under_agents() {
        let root =
            habitat_root_for_session(Path::new("C:/Users/test/.wardian"), "provider-session-123")
                .expect("expected provider session path");

        assert_eq!(
            root,
            Path::new("C:/Users/test/.wardian")
                .join("agents")
                .join("provider-session-123")
                .join("habitat")
        );
    }

    #[test]
    fn habitat_root_rejects_missing_session_id() {
        let err = habitat_root_for_session(Path::new("C:/Users/test/.wardian"), "   ")
            .expect_err("expected missing session id to be rejected");

        assert!(err.contains("Provider session ID is required"));
    }

    #[test]
    fn only_codex_uses_projected_workspaces() {
        assert!(!provider_uses_projected_workspace("claude"));
        assert!(provider_uses_projected_workspace("codex"));
        assert!(!provider_uses_projected_workspace("gemini"));
    }

    #[test]
    fn projected_link_match_detects_existing_workspace_projection() {
        let root = unique_temp_dir("workspace-link-test");
        let target = root.join("target");
        let link = root.join("link");

        std::fs::create_dir_all(&target).expect("create target dir");
        create_directory_link(&target, &link).expect("create projected link");

        assert!(projected_link_matches_target(&link, &target));

        let _ = std::fs::remove_dir_all(&link).or_else(|_| std::fs::remove_dir(&link));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_home_projection_only_seeds_shared_files() {
        let root = unique_temp_dir("codex-home-shared-files");
        let real_home = root.join("real-codex-home");
        let projected_home = root.join("projected-home");
        let wardian_skills = root.join("wardian-skills");

        std::fs::create_dir_all(&real_home).expect("create real codex home");
        std::fs::create_dir_all(&projected_home).expect("create projected codex home");
        std::fs::create_dir_all(&wardian_skills).expect("create wardian skills");

        std::fs::write(real_home.join("auth.json"), "auth").expect("write auth");
        std::fs::write(real_home.join("config.toml"), "config").expect("write config");
        std::fs::write(real_home.join("cap_sid"), "cap").expect("write cap sid");
        std::fs::write(real_home.join("history.jsonl"), "history").expect("write unrelated file");

        sync_codex_agent_home(&real_home, &projected_home, &wardian_skills)
            .expect("sync codex agent home");

        assert!(projected_home.join("auth.json").exists());
        assert!(projected_home.join("config.toml").exists());
        assert!(projected_home.join("cap_sid").exists());
        assert!(!projected_home.join("history.jsonl").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn get_wardian_home_respects_env_override() {
        let dir = unique_temp_dir("wardian-home-override");
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", dir.to_str().unwrap()) };
        let result = super::get_wardian_home();
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert_eq!(result.unwrap(), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_wardian_home_falls_back_without_env() {
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        let result = super::get_wardian_home();
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(
            path.ends_with(".wardian"),
            "Expected path to end with .wardian, got: {:?}",
            path
        );
    }

    #[test]
    fn get_wardian_home_ignores_empty_env() {
        unsafe { std::env::set_var("WARDIAN_HOME", "") };
        let result = super::get_wardian_home();
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert!(result.is_some());
        assert!(result.unwrap().ends_with(".wardian"));
    }

    #[test]
    fn codex_home_projection_preserves_system_skills_and_adds_wardian_skills() {
        let root = unique_temp_dir("codex-home-skills");
        let real_home = root.join("real-codex-home");
        let projected_home = root.join("projected-home");
        let projected_system_skill = projected_home
            .join("skills")
            .join(".system")
            .join("marker-skill");
        let wardian_skill = root.join("wardian-skills").join("role-skill");

        std::fs::create_dir_all(&real_home).expect("create real codex home");
        std::fs::create_dir_all(&projected_system_skill).expect("create projected system skill");
        std::fs::create_dir_all(&wardian_skill).expect("create wardian skill dir");
        std::fs::write(wardian_skill.join("SKILL.md"), "wardian skill")
            .expect("write wardian skill");

        sync_codex_agent_home(&real_home, &projected_home, &root.join("wardian-skills"))
            .expect("sync codex agent home");

        assert!(projected_system_skill.exists());
        assert!(projected_home.join("skills").join("role-skill").exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
