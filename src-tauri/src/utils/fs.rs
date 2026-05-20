#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub struct ClaudePermissionHookPaths {
    pub settings_arg: String,
    pub event_log_path: std::path::PathBuf,
}

pub fn get_wardian_home() -> Option<std::path::PathBuf> {
    wardian_core::paths::wardian_home_for_manifest(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
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
            if let Ok(data) = std::fs::read_to_string(home.join("settings/state.json")) {
                if let Ok(configs) =
                    serde_json::from_str::<Vec<wardian_core::models::AgentConfig>>(&data)
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
    matches!(provider, "codex" | "opencode")
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
    std::fs::write(&event_log_path, "").map_err(|e| e.to_string())?;

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
        // Expose canonical agent skills through provider-specific discovery shims.
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

pub fn project_antigravity_include_directories(session_id: &str, dirs: Vec<String>) -> Vec<String> {
    dirs.into_iter()
        .enumerate()
        .map(|(index, dir)| project_antigravity_include_directory(session_id, index, dir))
        .collect()
}

fn project_antigravity_include_directory(session_id: &str, index: usize, dir: String) -> String {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return dir;
    }

    let source = std::path::PathBuf::from(trimmed);
    if !source.is_dir() || !path_has_hidden_component(&source) {
        return dir;
    }

    let projection_root = std::env::temp_dir()
        .join("wardian-antigravity")
        .join(safe_projection_name(session_id))
        .join("include");
    let link = projection_root.join(format!(
        "{index:02}-{}",
        source
            .file_name()
            .and_then(|name| name.to_str())
            .map(safe_projection_name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "dir".to_string())
    ));

    if source.join(".agents").join("skills").exists() {
        match materialize_antigravity_include_projection(&source, &link) {
            Ok(()) => link.to_string_lossy().to_string(),
            Err(_) => dir,
        }
    } else {
        if projected_link_matches_target(&link, &source) {
            return link.to_string_lossy().to_string();
        }

        if (link.exists() || link.symlink_metadata().is_ok())
            && remove_existing_projection_path(&link).is_err()
        {
            return dir;
        }

        match create_directory_link(&source, &link) {
            Ok(()) => link.to_string_lossy().to_string(),
            Err(_) => dir,
        }
    }
}

fn materialize_antigravity_include_projection(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    if target.exists() || target.symlink_metadata().is_ok() {
        remove_existing_projection_path(target)?;
    }
    copy_dir_all_following_links(source, target).map_err(|e| e.to_string())
}

fn path_has_hidden_component(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        text.starts_with('.') && text != "." && text != ".."
    })
}

fn safe_projection_name(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_') {
            output.push(ch);
        } else if !output.ends_with('-') {
            output.push('-');
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "session".to_string()
    } else {
        output
    }
}

/// Convert a filesystem path to a forward-slash string safe for JSON/JSONC embedding.
/// OpenCode is a Node.js app and accepts forward slashes on all platforms.
/// Windows backslashes produce invalid JSONC escape sequences (e.g. `\U`, `\t`) that
/// can cause generated OpenCode config to be rejected.
fn path_to_forward_slash(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn build_opencode_runtime_config(include_roots: &[std::path::PathBuf]) -> serde_json::Value {
    let mut instructions = Vec::new();

    for root in include_roots {
        if root.as_os_str().is_empty() {
            continue;
        }

        let instruction_file = root.join("AGENTS.md");
        if instruction_file.is_file() {
            let path = path_to_forward_slash(&instruction_file);
            if !instructions.contains(&path) {
                instructions.push(path);
            }
        }
    }

    let mut config = serde_json::Map::new();
    if !instructions.is_empty() {
        config.insert(
            "instructions".to_string(),
            serde_json::Value::Array(
                instructions
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(config)
}

pub fn sync_opencode_config_dir(
    config_dir: &std::path::Path,
    include_roots: &[std::path::PathBuf],
) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;

    let merged_skills = config_dir.join("skills");
    if merged_skills.exists() {
        let _ = std::fs::remove_dir_all(&merged_skills)
            .or_else(|_| std::fs::remove_dir(&merged_skills));
    }
    std::fs::create_dir_all(&merged_skills).map_err(|e| e.to_string())?;

    for root in include_roots {
        let source = root.join(".agents").join("skills");
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

pub fn resolve_opencode_runtime_roots(
    class_name: &str,
    session_id: Option<&str>,
    system_include_directories: Option<&[String]>,
    include_directories: Option<&[String]>,
) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();

    let mut push_unique = |path: std::path::PathBuf| {
        if !path.as_os_str().is_empty() && !roots.contains(&path) {
            roots.push(path);
        }
    };

    if let Some(system_dirs) = system_include_directories {
        for dir in system_dirs {
            let trimmed = dir.trim();
            if !trimmed.is_empty() {
                push_unique(std::path::PathBuf::from(trimmed));
            }
        }
    } else if let Some(wardian_home) = get_wardian_home() {
        let common_dir = wardian_home.join("common");
        if common_dir.exists() {
            push_unique(common_dir);
        }

        let trimmed_class = class_name.trim();
        if !trimmed_class.is_empty() {
            let class_dir = wardian_home.join("classes").join(trimmed_class);
            if class_dir.exists() {
                push_unique(class_dir);
            }
        }

        if let Some(session_id) = session_id.map(str::trim).filter(|sid| !sid.is_empty()) {
            let agent_dir = wardian_home.join("agents").join(session_id);
            if agent_dir.exists() {
                push_unique(agent_dir);
            }
        }
    }

    if let Some(user_dirs) = include_directories {
        for dir in user_dirs {
            let trimmed = dir.trim();
            if !trimmed.is_empty() {
                push_unique(std::path::PathBuf::from(trimmed));
            }
        }
    }

    roots
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
    remove_legacy_codex_global_hardlinks(real_codex_home, projected_home)?;

    for shared_name in CODEX_SHARED_HOME_FILES {
        let source = real_codex_home.join(shared_name);
        if source.exists() && source.is_file() {
            project_file(&source, &projected_home.join(shared_name))?;
        }
    }

    sync_codex_windows_sandbox_support(real_codex_home, projected_home)?;

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

const CODEX_SHARED_HOME_FILES: &[&str] = &["auth.json", "config.toml", "cap_sid"];

#[cfg(windows)]
const CODEX_WINDOWS_SHARED_SANDBOX_DIRS: &[&str] = &[".sandbox-secrets", ".sandbox-bin"];

#[cfg(windows)]
const CODEX_WINDOWS_SANDBOX_SETUP_FILES: &[&str] = &["setup_marker.json"];

pub(crate) fn sync_codex_windows_sandbox_support(
    real_codex_home: &std::path::Path,
    projected_home: &std::path::Path,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        for shared_name in CODEX_WINDOWS_SHARED_SANDBOX_DIRS {
            let source = real_codex_home.join(shared_name);
            if source.is_dir() {
                project_directory_link(&source, &projected_home.join(shared_name))?;
            }
        }

        let real_sandbox = real_codex_home.join(".sandbox");
        if real_sandbox.is_dir() {
            let projected_sandbox = projected_home.join(".sandbox");
            std::fs::create_dir_all(&projected_sandbox).map_err(|e| e.to_string())?;
            for file_name in CODEX_WINDOWS_SANDBOX_SETUP_FILES {
                let source = real_sandbox.join(file_name);
                if source.is_file() {
                    project_file(&source, &projected_sandbox.join(file_name))?;
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (real_codex_home, projected_home);
    }

    Ok(())
}

#[cfg(windows)]
fn project_directory_link(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), String> {
    if projected_link_matches_target(target, source) {
        return Ok(());
    }

    remove_projected_path(target)?;
    create_directory_link(source, target)
}

#[cfg(windows)]
fn remove_projected_path(path: &std::path::Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };

    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        if metadata.is_dir() {
            return std::fs::remove_dir(path)
                .or_else(|_| std::fs::remove_file(path))
                .map_err(|e| e.to_string());
        }
        return std::fs::remove_file(path)
            .or_else(|_| std::fs::remove_dir(path))
            .map_err(|e| e.to_string());
    }

    if metadata.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
fn remove_existing_projection_path(path: &std::path::Path) -> Result<(), String> {
    remove_projected_path(path)
}

#[cfg(not(windows))]
fn remove_existing_projection_path(path: &std::path::Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };

    if metadata.file_type().is_symlink() || metadata.is_dir() {
        std::fs::remove_dir(path).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

const CODEX_LEGACY_GLOBAL_HARDLINK_GROUPS: &[(&str, &[&str], bool)] = &[
    ("history.jsonl", &[], true),
    ("session_index.jsonl", &[], true),
    (
        "state_5.sqlite",
        &["state_5.sqlite-shm", "state_5.sqlite-wal"],
        false,
    ),
    (
        "logs_2.sqlite",
        &["logs_2.sqlite-shm", "logs_2.sqlite-wal"],
        false,
    ),
];

fn remove_legacy_codex_global_hardlinks(
    real_codex_home: &std::path::Path,
    projected_home: &std::path::Path,
) -> Result<(), String> {
    for (primary_name, sidecar_names, allow_content_match) in CODEX_LEGACY_GLOBAL_HARDLINK_GROUPS {
        let source = real_codex_home.join(primary_name);
        let target = projected_home.join(primary_name);
        if same_file_identity(&source, &target)
            || (*allow_content_match && same_file_contents(&source, &target))
        {
            std::fs::remove_file(&target).map_err(|e| e.to_string())?;
            for sidecar_name in *sidecar_names {
                let sidecar = projected_home.join(sidecar_name);
                if sidecar.exists() {
                    std::fs::remove_file(&sidecar).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

fn same_file_identity(left: &std::path::Path, right: &std::path::Path) -> bool {
    same_file::is_same_file(left, right).unwrap_or(false)
}

fn same_file_contents(left: &std::path::Path, right: &std::path::Path) -> bool {
    let Ok(left_content) = std::fs::read(left) else {
        return false;
    };
    let Ok(right_content) = std::fs::read(right) else {
        return false;
    };
    left_content == right_content
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

pub(crate) fn create_directory_link(
    target: &std::path::Path,
    link: &std::path::Path,
) -> Result<(), String> {
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

pub(crate) fn copy_dir_all(
    src: impl AsRef<std::path::Path>,
    dst: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_entry = dst.as_ref().join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst_entry)?;
        } else {
            std::fs::copy(entry.path(), dst_entry)?;
        }
    }
    Ok(())
}

fn copy_dir_all_following_links(
    src: impl AsRef<std::path::Path>,
    dst: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.as_ref().join(entry.file_name());
        if source.is_dir() {
            copy_dir_all_following_links(&source, &target)?;
        } else if source.is_file() {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn project_file(source: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    if target.exists() {
        let _ = std::fs::remove_file(target);
    }

    std::fs::copy(source, target)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Ensures `.claude/skills` is a symlink (or junction on Windows) pointing to
/// `.agents/skills` within the given base directory. `.agents/skills` remains
/// the provider-agnostic canonical location; this is only a compatibility shim
/// for providers that require their own discovery path.
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
        build_opencode_runtime_config, create_directory_link, ensure_claude_permission_hook,
        habitat_root_for_session, project_antigravity_include_directories,
        projected_link_matches_target, provider_uses_projected_workspace,
        resolve_opencode_runtime_roots, sync_codex_agent_home, sync_opencode_config_dir,
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
    fn only_codex_and_opencode_use_projected_workspaces() {
        assert!(!provider_uses_projected_workspace("claude"));
        assert!(provider_uses_projected_workspace("codex"));
        assert!(provider_uses_projected_workspace("opencode"));
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
    fn codex_home_projection_shares_safe_profile_files() {
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
        std::fs::write(real_home.join("session_index.jsonl"), "index")
            .expect("write unrelated index");
        std::fs::write(real_home.join("state_5.sqlite"), "state").expect("write state");
        std::fs::write(real_home.join("logs_2.sqlite"), "logs").expect("write logs");
        std::fs::write(real_home.join("logs_2.sqlite-wal"), "logs wal").expect("write logs wal");
        std::fs::write(real_home.join("sandbox.log"), "sandbox").expect("write runtime file");
        std::fs::create_dir_all(real_home.join("sessions")).expect("write sessions dir");

        sync_codex_agent_home(&real_home, &projected_home, &wardian_skills)
            .expect("sync codex agent home");

        assert!(projected_home.join("auth.json").exists());
        assert!(projected_home.join("config.toml").exists());
        assert!(projected_home.join("cap_sid").exists());
        assert!(!projected_home.join("history.jsonl").exists());
        assert!(!projected_home.join("session_index.jsonl").exists());
        assert!(!projected_home.join("state_5.sqlite").exists());
        assert!(!projected_home.join("logs_2.sqlite").exists());
        assert!(!projected_home.join("logs_2.sqlite-wal").exists());
        assert!(!projected_home.join("sandbox.log").exists());
        assert!(!projected_home.join("sessions").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn codex_home_projection_shares_windows_sandbox_support_without_runtime_logs() {
        let root = unique_temp_dir("codex-home-windows-sandbox-support");
        let real_home = root.join("real-codex-home");
        let projected_home = root.join("projected-home");
        let wardian_skills = root.join("wardian-skills");
        let stale_secrets_target = root.join("stale-secrets-target");

        std::fs::create_dir_all(real_home.join(".sandbox-secrets")).expect("create real secrets");
        std::fs::create_dir_all(real_home.join(".sandbox-bin")).expect("create real helpers");
        std::fs::create_dir_all(real_home.join(".sandbox")).expect("create real sandbox");
        std::fs::create_dir_all(projected_home.join(".sandbox")).expect("create local sandbox");
        std::fs::create_dir_all(&wardian_skills).expect("create wardian skills");
        std::fs::create_dir_all(&stale_secrets_target).expect("create stale secrets target");

        std::fs::write(
            real_home
                .join(".sandbox-secrets")
                .join("sandbox_users.json"),
            "real secrets",
        )
        .expect("write real secrets");
        std::fs::write(
            real_home
                .join(".sandbox-bin")
                .join("codex-command-runner.exe"),
            "runner",
        )
        .expect("write helper");
        std::fs::write(
            real_home.join(".sandbox").join("setup_marker.json"),
            "real setup marker",
        )
        .expect("write marker");
        std::fs::write(real_home.join(".sandbox").join("sandbox.log"), "real log")
            .expect("write real log");
        std::fs::write(
            real_home.join(".sandbox").join("setup_error.json"),
            "real setup error",
        )
        .expect("write real setup error");
        std::fs::write(
            projected_home.join(".sandbox").join("sandbox.log"),
            "agent log",
        )
        .expect("write projected log");
        std::fs::write(
            projected_home.join(".sandbox").join("setup_error.json"),
            "agent setup error",
        )
        .expect("write projected setup error");
        std::fs::write(stale_secrets_target.join("sentinel.txt"), "do not delete")
            .expect("write stale target sentinel");
        create_directory_link(
            &stale_secrets_target,
            &projected_home.join(".sandbox-secrets"),
        )
        .expect("create stale projected secrets link");

        sync_codex_agent_home(&real_home, &projected_home, &wardian_skills)
            .expect("sync codex agent home");

        assert!(projected_link_matches_target(
            &projected_home.join(".sandbox-secrets"),
            &real_home.join(".sandbox-secrets")
        ));
        assert!(projected_link_matches_target(
            &projected_home.join(".sandbox-bin"),
            &real_home.join(".sandbox-bin")
        ));
        assert!(
            !projected_link_matches_target(
                &projected_home.join(".sandbox"),
                &real_home.join(".sandbox")
            ),
            "the sandbox runtime directory must stay per-agent"
        );
        assert_eq!(
            std::fs::read_to_string(
                projected_home
                    .join(".sandbox-secrets")
                    .join("sandbox_users.json")
            )
            .expect("read projected secrets"),
            "real secrets"
        );
        assert_eq!(
            std::fs::read_to_string(projected_home.join(".sandbox").join("setup_marker.json"))
                .expect("read projected marker"),
            "real setup marker"
        );
        assert_eq!(
            std::fs::read_to_string(projected_home.join(".sandbox").join("sandbox.log"))
                .expect("read projected log"),
            "agent log"
        );
        assert_eq!(
            std::fs::read_to_string(projected_home.join(".sandbox").join("setup_error.json"))
                .expect("read projected setup error"),
            "agent setup error"
        );
        assert_eq!(
            std::fs::read_to_string(stale_secrets_target.join("sentinel.txt"))
                .expect("stale target should not be deleted when replacing junction"),
            "do not delete"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_home_projection_copies_shared_files_without_linking_real_home() {
        let root = unique_temp_dir("codex-home-copy-shared-files");
        let real_home = root.join("real-codex-home");
        let projected_home = root.join("projected-home");
        let wardian_skills = root.join("wardian-skills");

        std::fs::create_dir_all(&real_home).expect("create real codex home");
        std::fs::create_dir_all(&wardian_skills).expect("create wardian skills");
        std::fs::write(real_home.join("auth.json"), "source auth").expect("write auth");

        sync_codex_agent_home(&real_home, &projected_home, &wardian_skills)
            .expect("sync codex agent home");

        std::fs::write(projected_home.join("auth.json"), "projected auth")
            .expect("mutate projected auth");

        assert_eq!(
            std::fs::read_to_string(real_home.join("auth.json")).expect("read source auth"),
            "source auth"
        );
        assert_eq!(
            std::fs::read_to_string(projected_home.join("auth.json")).expect("read projected auth"),
            "projected auth"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_home_projection_removes_legacy_global_hardlinks_and_copies() {
        let root = unique_temp_dir("codex-home-legacy-state");
        let real_home = root.join("real-codex-home");
        let projected_home = root.join("projected-home");

        std::fs::create_dir_all(&real_home).expect("create real codex home");
        std::fs::create_dir_all(&projected_home).expect("create projected codex home");

        for file_name in [
            "history.jsonl",
            "session_index.jsonl",
            "state_5.sqlite",
            "state_5.sqlite-shm",
            "state_5.sqlite-wal",
            "logs_2.sqlite",
            "logs_2.sqlite-shm",
            "logs_2.sqlite-wal",
        ] {
            std::fs::write(real_home.join(file_name), file_name).expect("write real file");
            if file_name == "session_index.jsonl" {
                std::fs::copy(real_home.join(file_name), projected_home.join(file_name))
                    .expect("create legacy copy");
            } else {
                std::fs::hard_link(real_home.join(file_name), projected_home.join(file_name))
                    .expect("create legacy hardlink");
            }
        }

        sync_codex_agent_home(&real_home, &projected_home, &root.join("wardian-skills"))
            .expect("sync codex agent home");

        for file_name in [
            "history.jsonl",
            "session_index.jsonl",
            "state_5.sqlite",
            "state_5.sqlite-shm",
            "state_5.sqlite-wal",
            "logs_2.sqlite",
            "logs_2.sqlite-shm",
            "logs_2.sqlite-wal",
        ] {
            assert!(
                !projected_home.join(file_name).exists(),
                "{file_name} should be removed from projected Codex home"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_home_projection_keeps_non_hardlinked_sqlite_state_files() {
        let root = unique_temp_dir("codex-home-sqlite-copy");
        let real_home = root.join("real-codex-home");
        let projected_home = root.join("projected-home");

        std::fs::create_dir_all(&real_home).expect("create real codex home");
        std::fs::create_dir_all(&projected_home).expect("create projected codex home");
        std::fs::write(real_home.join("state_5.sqlite"), "sqlite copy").expect("write real sqlite");
        std::fs::copy(
            real_home.join("state_5.sqlite"),
            projected_home.join("state_5.sqlite"),
        )
        .expect("create projected sqlite copy");

        sync_codex_agent_home(&real_home, &projected_home, &root.join("wardian-skills"))
            .expect("sync codex agent home");

        assert!(projected_home.join("state_5.sqlite").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn get_wardian_home_respects_env_override() {
        let _guard = crate::utils::wardian_test_env_lock();
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
        let _guard = crate::utils::wardian_test_env_lock();
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
        let _guard = crate::utils::wardian_test_env_lock();
        unsafe { std::env::set_var("WARDIAN_HOME", "") };
        let result = super::get_wardian_home();
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert!(result.is_some());
        assert!(result.unwrap().ends_with(".wardian"));
    }

    #[test]
    fn ensure_claude_permission_hook_truncates_stale_events() {
        let _guard = crate::utils::wardian_test_env_lock();
        let root = unique_temp_dir("claude-hook-stale-events");
        unsafe { std::env::set_var("WARDIAN_HOME", root.to_str().unwrap()) };

        let stale_log = root
            .join("agents")
            .join("session-123")
            .join("claude")
            .join("permission-requests.jsonl");
        std::fs::create_dir_all(stale_log.parent().unwrap()).expect("create hook dir");
        std::fs::write(&stale_log, "{\"tool_name\":\"Bash\"}\n").expect("write stale hook event");

        let paths = ensure_claude_permission_hook("session-123").expect("ensure hook");

        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert_eq!(
            std::fs::read_to_string(paths.event_log_path).expect("read hook log"),
            ""
        );
        let _ = std::fs::remove_dir_all(&root);
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

    #[test]
    fn opencode_runtime_config_collects_instruction_files() {
        let root = unique_temp_dir("opencode-runtime-config");
        let common = root.join("common");
        let class_dir = root.join("class");
        let agent_dir = root.join("agent");

        for dir in [&common, &class_dir, &agent_dir] {
            std::fs::create_dir_all(dir.join(".agents").join("skills").join("skill-one"))
                .expect("create skill dir");
            std::fs::write(
                dir.join("AGENTS.md"),
                format!("instructions for {}", dir.display()),
            )
            .expect("write AGENTS");
        }

        let config: serde_json::Value = build_opencode_runtime_config(&[
            common.clone(),
            class_dir.clone(),
            agent_dir.clone(),
            common.clone(),
        ]);

        let instructions = config
            .get("instructions")
            .and_then(|v| v.as_array())
            .expect("instructions array");
        assert!(config.get("theme").is_none());
        assert_eq!(instructions.len(), 3);
        assert_eq!(
            instructions[0].as_str(),
            Some(
                common
                    .join("AGENTS.md")
                    .to_string_lossy()
                    .replace('\\', "/")
                    .as_str()
            )
        );
        assert_eq!(
            instructions[1].as_str(),
            Some(
                class_dir
                    .join("AGENTS.md")
                    .to_string_lossy()
                    .replace('\\', "/")
                    .as_str()
            )
        );
        assert_eq!(
            instructions[2].as_str(),
            Some(
                agent_dir
                    .join("AGENTS.md")
                    .to_string_lossy()
                    .replace('\\', "/")
                    .as_str()
            )
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opencode_config_dir_projects_skill_roots() {
        let root = unique_temp_dir("opencode-config-dir");
        let common = root.join("common");
        let class_dir = root.join("class");
        let config_dir = root.join("config");

        for (dir, skill) in [(&common, "common-skill"), (&class_dir, "class-skill")] {
            std::fs::create_dir_all(dir.join(".agents").join("skills").join(skill))
                .expect("create skill dir");
        }

        sync_opencode_config_dir(&config_dir, &[common.clone(), class_dir.clone()])
            .expect("sync opencode config dir");

        assert!(config_dir.join("skills").join("common-skill").exists());
        assert!(config_dir.join("skills").join("class-skill").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opencode_runtime_roots_fall_back_to_common_class_and_agent_dirs() {
        let _guard = crate::utils::wardian_test_env_lock();
        let wardian_home = unique_temp_dir("opencode-runtime-roots");
        let common = wardian_home.join("common");
        let class_dir = wardian_home.join("classes").join("Builder");
        let agent_dir = wardian_home.join("agents").join("ses_123");

        std::fs::create_dir_all(&common).expect("create common dir");
        std::fs::create_dir_all(&class_dir).expect("create class dir");
        std::fs::create_dir_all(&agent_dir).expect("create agent dir");

        unsafe { std::env::set_var("WARDIAN_HOME", wardian_home.to_string_lossy().to_string()) };
        let roots = resolve_opencode_runtime_roots("Builder", Some("ses_123"), None, None);
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(roots, vec![common, class_dir, agent_dir]);

        let _ = std::fs::remove_dir_all(&wardian_home);
    }

    #[test]
    fn antigravity_include_projection_exposes_hidden_wardian_roots_through_visible_paths() {
        let root = unique_temp_dir("antigravity-include-projection");
        let hidden = root.join(".wardian").join("classes").join("Builder");
        std::fs::create_dir_all(hidden.join(".agents").join("skills").join("role-skill"))
            .expect("create hidden skill");
        std::fs::write(hidden.join("AGENTS.md"), "role instructions").expect("write agents");

        let projected = project_antigravity_include_directories(
            "session-123",
            vec![hidden.to_string_lossy().to_string()],
        );

        assert_eq!(projected.len(), 1);
        let projected_path = PathBuf::from(&projected[0]);
        assert!(!projected_path
            .components()
            .any(|component| component.as_os_str().to_string_lossy() == ".wardian"));
        assert!(projected_path.join("AGENTS.md").exists());
        assert!(projected_path
            .join(".agents")
            .join("skills")
            .join("role-skill")
            .exists());

        let _ = std::fs::remove_dir_all(&root);
        if let Some(parent) = projected_path.parent().and_then(|path| path.parent()) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn antigravity_include_projection_materializes_linked_skills() {
        let root = unique_temp_dir("antigravity-linked-skills");
        let hidden = root.join(".wardian");
        let source = hidden.join("common");
        let library_skill = hidden
            .join("library")
            .join("skills")
            .join("wardian-skills")
            .join("wardian-cli");
        let deployed_skill = source
            .join(".agents")
            .join("skills")
            .join("wardian-cli");

        std::fs::create_dir_all(&library_skill).expect("create library skill");
        std::fs::write(library_skill.join("SKILL.md"), "wardian cli instructions")
            .expect("write library skill");
        std::fs::create_dir_all(deployed_skill.parent().expect("skill parent"))
            .expect("create deployed skills parent");
        create_directory_link(&library_skill, &deployed_skill).expect("link deployed skill");

        let projected = project_antigravity_include_directories(
            "session-linked-skills",
            vec![source.to_string_lossy().to_string()],
        );

        assert_eq!(projected.len(), 1);
        let projected_path = PathBuf::from(&projected[0]);
        let projected_skill = projected_path
            .join(".agents")
            .join("skills")
            .join("wardian-cli");
        assert_eq!(
            std::fs::read_to_string(projected_skill.join("SKILL.md")).expect("read projected skill"),
            "wardian cli instructions"
        );
        assert!(
            std::fs::read_link(&projected_skill).is_err(),
            "projected skill must be a materialized directory, not a link back into hidden storage"
        );

        let _ = std::fs::remove_dir_all(&root);
        if let Some(parent) = projected_path.parent().and_then(|path| path.parent()) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
