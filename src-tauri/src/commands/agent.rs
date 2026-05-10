use crate::manager;
use crate::state::AppState;
use tauri::{AppHandle, Emitter, State};
use wardian_core::models::{
    AgentConfig, AgentSessionPersistence, AgentSessionPersistenceOverride, AgentTelemetry,
    DeployedSkillRef,
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnAgentRequest {
    pub session_name: String,
    pub agent_class: String,
    pub folder: String,
    pub resume_session: Option<String>,
    pub is_off: Option<bool>,
    pub config_override: Option<AgentConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneAgentMode {
    Fresh,
    Profile,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CloneAgentRequest {
    pub source_session_id: String,
    pub mode: CloneAgentMode,
    pub session_name: Option<String>,
    pub provider: Option<String>,
    pub folder: Option<String>,
    pub agent_class: Option<String>,
    pub start: Option<bool>,
    pub profile_selection: Option<CloneProfileSelection>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CloneFileTreeNode {
    pub name: String,
    pub path: String,
    pub kind: CloneFileTreeNodeKind,
    pub children: Vec<CloneFileTreeNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneFileTreeNodeKind {
    File,
    Directory,
}

impl CloneFileTreeNode {
    #[cfg(test)]
    fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentClonePreview {
    pub source_session_id: String,
    pub source_session_name: String,
    pub suggested_session_name: String,
    pub provider: String,
    pub agent_class: String,
    pub folder: String,
    pub files: CloneFileTreeNode,
    pub default_selected_files: Vec<String>,
    pub skills: Vec<DeployedSkillRef>,
    pub default_selected_skills: Vec<DeployedSkillRef>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CloneProfileSelection {
    pub files: Vec<String>,
    pub skills: Vec<DeployedSkillRef>,
}

fn clone_unique_name(
    source_name: &str,
    existing_names: &std::collections::HashSet<String>,
) -> String {
    let base = if source_name.trim().is_empty() {
        "agent"
    } else {
        source_name.trim()
    };
    let first = format!("{}-copy", base);
    if !existing_names.contains(&first) {
        return first;
    }

    let mut index = 2;
    loop {
        let candidate = format!("{}-copy-{}", base, index);
        if !existing_names.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn clone_quote_custom_arg(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-./:=,+".contains(ch))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}

fn clone_custom_args_without_provider_memory(custom_args: Option<&str>) -> Option<String> {
    let parsed = shlex::split(custom_args?.trim())?;
    let mut filtered = Vec::with_capacity(parsed.len());
    let mut iter = parsed.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if matches!(
            arg.as_str(),
            "--resume" | "--session" | "--session-id" | "-r"
        ) {
            let _ = iter.next();
            continue;
        }
        if matches!(arg.as_str(), "--continue" | "-c") {
            continue;
        }
        if arg.starts_with("--resume=")
            || arg.starts_with("--session=")
            || arg.starts_with("--session-id=")
            || arg.starts_with("-r=")
        {
            continue;
        }
        if arg == "resume" {
            let _ = iter.next_if(|next| !next.starts_with('-'));
            continue;
        }
        filtered.push(arg);
    }

    (!filtered.is_empty()).then(|| {
        filtered
            .iter()
            .map(|arg| clone_quote_custom_arg(arg))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn clone_sanitize_config(
    source: &AgentConfig,
    session_name: String,
    provider: Option<String>,
    folder: Option<String>,
    agent_class: Option<String>,
    start: bool,
) -> AgentConfig {
    let mut config = source.clone();
    config.session_id.clear();
    config.session_name = session_name;
    if let Some(provider) = provider.filter(|value| !value.trim().is_empty()) {
        config.provider = provider;
    }
    if let Some(folder) = folder {
        config.folder = folder;
    }
    if let Some(agent_class) = agent_class.filter(|value| !value.trim().is_empty()) {
        config.agent_class = agent_class;
    }
    config.resume_session = None;
    config.fresh_provider_session_id = None;
    config.codex_cleared_provider_sessions.clear();
    config.system_include_directories = None;
    config.opencode_port = None;
    config.custom_args = clone_custom_args_without_provider_memory(config.custom_args.as_deref());
    config.is_off = !start;
    config
}

fn clone_copy_file(source: &std::path::Path, destination: &std::path::Path) -> Result<(), String> {
    if !source.is_file() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn clone_remove_existing_path(path: &std::path::Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };

    if metadata.file_type().is_symlink() || std::fs::read_link(path).is_ok() {
        let _ = std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path));
        return;
    }

    if metadata.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                clone_remove_existing_path(&entry.path());
            }
        }
        let _ = std::fs::remove_dir(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

fn clone_resolve_link_target(
    link_path: &std::path::Path,
    link_target: std::path::PathBuf,
) -> std::path::PathBuf {
    if link_target.is_absolute() {
        link_target
    } else {
        link_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
            .join(link_target)
    }
}

fn clone_copy_link_or_target(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<bool, String> {
    let Ok(link_target) = std::fs::read_link(source) else {
        return Ok(false);
    };
    let resolved_target = clone_resolve_link_target(source, link_target);
    clone_remove_existing_path(destination);
    if resolved_target.is_dir() {
        crate::utils::fs::create_directory_link(&resolved_target, destination)?;
    } else if resolved_target.is_file() {
        clone_copy_file(&resolved_target, destination)?;
    }
    Ok(true)
}

fn clone_copy_directory_recursive(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    clone_remove_existing_path(destination);
    std::fs::create_dir_all(destination).map_err(|e| e.to_string())?;

    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if clone_copy_link_or_target(&source_path, &destination_path)? {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&source_path).map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            clone_copy_directory_recursive(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            clone_copy_file(&source_path, &destination_path)?;
        }
    }

    Ok(())
}

fn clone_copy_agent_profile_files(
    wardian_home: &std::path::Path,
    source_session_id: &str,
    destination_session_id: &str,
) -> Result<(), String> {
    let source_root = wardian_home.join("agents").join(source_session_id);
    let destination_root = wardian_home.join("agents").join(destination_session_id);
    std::fs::create_dir_all(&destination_root).map_err(|e| e.to_string())?;

    clone_copy_file(
        &source_root.join("AGENTS.md"),
        &destination_root.join("AGENTS.md"),
    )?;
    clone_copy_directory_recursive(
        &source_root.join(".agents").join("skills"),
        &destination_root.join(".agents").join("skills"),
    )
}

fn clone_collect_eligible_file_tree(
    wardian_home: &std::path::Path,
    source_session_id: &str,
) -> Result<CloneFileTreeNode, String> {
    let source_root = wardian_home.join("agents").join(source_session_id);
    let canonical_root = source_root.canonicalize().map_err(|e| e.to_string())?;
    let children = clone_collect_eligible_file_children(&source_root, &canonical_root, "")?;

    Ok(CloneFileTreeNode {
        name: source_session_id.to_string(),
        path: String::new(),
        kind: CloneFileTreeNodeKind::Directory,
        children,
    })
}

fn clone_collect_eligible_file_children(
    directory: &std::path::Path,
    canonical_root: &std::path::Path,
    rel_dir: &str,
) -> Result<Vec<CloneFileTreeNode>, String> {
    let mut children = Vec::new();

    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_path = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        if clone_path_is_generated_or_runtime(&rel_path) {
            continue;
        }

        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if clone_path_is_link_or_reparse(&metadata) || std::fs::read_link(&path).is_ok() {
            continue;
        }
        let canonical_path = match path.canonicalize() {
            Ok(canonical_path) => canonical_path,
            Err(_) => continue,
        };
        if !canonical_path.starts_with(canonical_root) {
            continue;
        }

        if metadata.is_dir() {
            let nested = clone_collect_eligible_file_children(&path, canonical_root, &rel_path)?;
            if !nested.is_empty() {
                children.push(CloneFileTreeNode {
                    name,
                    path: rel_path,
                    kind: CloneFileTreeNodeKind::Directory,
                    children: nested,
                });
            }
        } else if metadata.is_file() {
            children.push(CloneFileTreeNode {
                name,
                path: rel_path,
                kind: CloneFileTreeNodeKind::File,
                children: Vec::new(),
            });
        }
    }

    children.sort_by(|a, b| {
        let kind_order = |kind: CloneFileTreeNodeKind| match kind {
            CloneFileTreeNodeKind::Directory => 0,
            CloneFileTreeNodeKind::File => 1,
        };
        kind_order(a.kind)
            .cmp(&kind_order(b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(children)
}

fn clone_path_is_generated_or_runtime(rel_path: &str) -> bool {
    let normalized = rel_path.replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or_default();
    matches!(first, "habitat" | ".codex" | ".claude" | ".gemini" | ".opencode")
        || normalized == ".agents/skills"
        || normalized.starts_with(".agents/skills/")
}

#[cfg(windows)]
fn clone_path_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn clone_path_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn flatten_clone_file_paths(root: &CloneFileTreeNode) -> Vec<String> {
    fn walk(node: &CloneFileTreeNode, output: &mut Vec<String>) {
        if node.kind == CloneFileTreeNodeKind::File {
            output.push(node.path.clone());
        }
        for child in &node.children {
            walk(child, output);
        }
    }

    let mut paths = Vec::new();
    walk(root, &mut paths);
    paths
}

fn build_agent_clone_preview(
    wardian_home: &std::path::Path,
    source_session_id: &str,
    source_config: &AgentConfig,
    existing_names: &std::collections::HashSet<String>,
) -> Result<AgentClonePreview, String> {
    let files = clone_collect_eligible_file_tree(wardian_home, source_session_id)?;
    let default_selected_files = flatten_clone_file_paths(&files)
        .into_iter()
        .filter(|path| path == "AGENTS.md")
        .collect::<Vec<_>>();
    let skills =
        crate::commands::library::list_deployed_skill_refs_for_target("agent", source_session_id)?;

    Ok(AgentClonePreview {
        source_session_id: source_session_id.to_string(),
        source_session_name: source_config.session_name.clone(),
        suggested_session_name: clone_unique_name(&source_config.session_name, existing_names),
        provider: source_config.provider.clone(),
        agent_class: source_config.agent_class.clone(),
        folder: source_config.folder.clone(),
        files,
        default_selected_files,
        default_selected_skills: skills.clone(),
        skills,
    })
}

fn clone_ensure_profile_destination_available(
    wardian_home: &std::path::Path,
    destination_session_id: &str,
    allowed_existing_session_id: Option<&str>,
) -> Result<(), String> {
    if allowed_existing_session_id == Some(destination_session_id) {
        return Ok(());
    }
    let destination_root = wardian_home.join("agents").join(destination_session_id);
    if destination_root.exists() || destination_root.symlink_metadata().is_ok() {
        return Err(format!(
            "Cannot clone profile into existing agent directory '{}'.",
            destination_root.display()
        ));
    }
    Ok(())
}

fn persisted_resume_session_for_provider(
    provider_name: &str,
    actual_resume: Option<String>,
    session_id: &str,
) -> Option<String> {
    actual_resume.or_else(|| {
        if provider_name == "claude" && !session_id.trim().is_empty() {
            Some(session_id.to_string())
        } else {
            None
        }
    })
}

fn provider_uses_generated_session_id(provider_name: &str) -> bool {
    matches!(provider_name, "claude" | "codex" | "mock")
}

fn provider_needs_obtain_session_id_on_clear(provider_name: &str) -> bool {
    matches!(provider_name, "gemini")
}

fn restore_runtime_state_snapshot_after_resume(
    new_active: &mut crate::state::ActiveAgent,
    query_count: usize,
    init_timestamp: Option<String>,
    log_path: Option<std::path::PathBuf>,
) {
    if let Ok(mut new_count) = new_active.query_count.lock() {
        *new_count = query_count;
    }
    if let Ok(mut new_ts) = new_active.init_timestamp.lock() {
        *new_ts = init_timestamp;
    }
    if let Ok(mut new_path) = new_active.log_path.lock() {
        *new_path = log_path;
    }
}

fn promote_fresh_provider_session_after_resume(
    provider: &str,
    new_active: &mut crate::state::ActiveAgent,
) {
    if provider != "claude" {
        return;
    }

    let mut new_config = new_active.config.lock().unwrap();
    if let Some(fresh_provider_session_id) = new_config.fresh_provider_session_id.take() {
        new_config.resume_session = Some(fresh_provider_session_id);
    }
}

fn sync_resumed_input_sender(
    state: &AppState,
    session_id: &str,
    stdin_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
) {
    if let Ok(mut senders) = state.input_senders.write() {
        if let Some(tx) = stdin_tx {
            senders.insert(session_id.to_string(), tx);
        } else {
            senders.remove(session_id);
        }
    }
}

fn agent_status_update_payload(session_id: &str, current_status: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "current_status": current_status,
    })
}

struct ResumeRuntimeSnapshot {
    config: AgentConfig,
    init_timestamp: Option<String>,
    query_count: usize,
    log_path: Option<std::path::PathBuf>,
}

fn capture_resume_runtime_snapshot(agent: &crate::state::ActiveAgent) -> ResumeRuntimeSnapshot {
    ResumeRuntimeSnapshot {
        config: agent.config.lock().unwrap().clone(),
        init_timestamp: agent.init_timestamp.lock().unwrap().clone(),
        query_count: agent.query_count.lock().map(|count| *count).unwrap_or(0),
        log_path: agent.log_path.lock().ok().and_then(|path| path.clone()),
    }
}

fn capture_opencode_pause_resume_session(agent: &crate::state::ActiveAgent) {
    let provider = {
        let config = agent.config.lock().unwrap();
        config.provider.clone()
    };

    if provider != "opencode" {
        return;
    }

    let mut config = agent.config.lock().unwrap();
    if config
        .resume_session
        .as_deref()
        .map(|s| !s.starts_with("ses_"))
        .unwrap_or(true)
    {
        let log_path_snap = agent.log_path.lock().ok().and_then(|guard| guard.clone());
        if let Some(log_path) = log_path_snap {
            if let Some(ses_id) = manager::opencode_extract_created_session_id(&log_path) {
                config.resume_session = Some(ses_id);
            }
        }
    }
}

fn mark_agent_paused_off(agent: &mut crate::state::ActiveAgent) {
    agent.pty_master = None;
    agent.stdin_tx = None;
    {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
    }
    if let Ok(mut status) = agent.current_status.lock() {
        *status = "Off".to_string();
    }
}

fn is_valid_name(name: &str) -> bool {
    let re = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();
    re.is_match(name)
}

fn generated_agent_name(
    agent_class: &str,
    existing_names: &std::collections::HashSet<String>,
) -> String {
    generated_agent_name_from_base(&generated_agent_name_base(agent_class), existing_names)
}

fn generated_agent_name_base(agent_class: &str) -> String {
    let mut previous_was_separator = false;
    let mut base = String::new();

    for ch in agent_class.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
            previous_was_separator = false;
        } else if !previous_was_separator && !base.is_empty() {
            base.push('-');
            previous_was_separator = true;
        }
    }

    let base = base.trim_matches('-');
    if base.is_empty() {
        "Agent".to_string()
    } else {
        base.to_string()
    }
}

fn generated_agent_name_from_base(
    base: &str,
    existing_names: &std::collections::HashSet<String>,
) -> String {
    let mut index = 1;
    loop {
        let candidate = format!("{}-{}", base, index);
        if !existing_names.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn invalid_agent_name_error() -> String {
    "Invalid agent name. Names must contain only alphanumeric characters, underscores, or hyphens (no spaces).".to_string()
}

fn resolve_requested_spawn_session_name(
    requested_session_name: &str,
    agent_class: &str,
    existing_names: &std::collections::HashSet<String>,
) -> Result<String, String> {
    if requested_session_name.trim().is_empty() {
        return Ok(generated_agent_name(agent_class, existing_names));
    }

    if !is_valid_name(requested_session_name) {
        return Err(invalid_agent_name_error());
    }

    if existing_names.contains(requested_session_name) {
        return Err(format!(
            "An agent with the name '{}' already exists.",
            requested_session_name
        ));
    }

    Ok(requested_session_name.to_string())
}

fn resolve_registered_session_name(
    session_name: &str,
    clone_name_base: Option<&str>,
    existing_names: &std::collections::HashSet<String>,
) -> Result<String, String> {
    if !existing_names.contains(session_name) {
        return Ok(session_name.to_string());
    }

    if let Some(base) = clone_name_base {
        return Ok(clone_unique_name(base, existing_names));
    }

    Err(format!(
        "An agent with the name '{}' already exists.",
        session_name
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentOrderPlacement<'a> {
    Top,
    After(&'a str),
}

fn insert_new_agent_order(
    order: &mut Vec<String>,
    session_id: &str,
    placement: AgentOrderPlacement<'_>,
) {
    order.retain(|id| id != session_id);
    match placement {
        AgentOrderPlacement::Top => order.insert(0, session_id.to_string()),
        AgentOrderPlacement::After(source_session_id) => {
            let index = order
                .iter()
                .position(|id| id == source_session_id)
                .map_or(0, |index| index + 1);
            order.insert(index, session_id.to_string());
        }
    }
}

struct SpawnNameReservation {
    session_name: String,
}

async fn reserve_spawn_session_name(
    state: &AppState,
    requested_session_name: &str,
    agent_class: &str,
) -> Result<SpawnNameReservation, String> {
    let agents = state.agents.lock().await;
    let mut existing_names = agents
        .values()
        .map(|agent| agent.config.lock().unwrap().session_name.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut reservations = state.agent_name_reservations.lock().await;
    existing_names.extend(reservations.iter().cloned());
    let session_name =
        resolve_requested_spawn_session_name(requested_session_name, agent_class, &existing_names)?;
    reservations.insert(session_name.clone());
    Ok(SpawnNameReservation { session_name })
}

async fn release_spawn_name_reservation(state: &AppState, session_name: &str) {
    let mut reservations = state.agent_name_reservations.lock().await;
    reservations.remove(session_name);
}

fn normalize_workspace_record_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_spawn_folder(folder: &str) -> Result<String, String> {
    if folder.trim().is_empty() {
        return Ok(String::new());
    }

    crate::utils::fs::validate_workspace_path(std::path::Path::new(folder))
        .map(|path| normalize_workspace_record_path(&path))
}

fn prepare_resume_config(config: &mut AgentConfig) -> Result<(), String> {
    config.is_off = false;

    let settings = crate::utils::load_shell_settings().unwrap_or_default();
    let resolved_persistence = match config.session_persistence {
        AgentSessionPersistenceOverride::Default => settings.agent_session_persistence,
        AgentSessionPersistenceOverride::Fresh => AgentSessionPersistence::Fresh,
        AgentSessionPersistenceOverride::Resume => AgentSessionPersistence::Resume,
    };

    config.fresh_provider_session_id = None;
    if resolved_persistence == AgentSessionPersistence::Fresh {
        config.resume_session = None;
        if config.provider == "claude" {
            config.fresh_provider_session_id = Some(uuid::Uuid::new_v4().to_string());
        }
        return Ok(());
    }

    // For opencode, Wardian uses a UUID as session_id internally, but opencode
    // only recognises real ses_xxx IDs.  Clear any stale UUID stored in
    // resume_session (e.g. from a pre-fix save), then only fall back to
    // session_id if it is already in the ses_xxx format.
    if config.provider == "opencode" {
        if let Some(ref rs) = config.resume_session {
            if !rs.starts_with("ses_") {
                config.resume_session = None;
            }
        }
    } else if config.provider == "codex"
        && config.resume_session.as_deref().is_some_and(|rs| {
            rs.trim().is_empty()
                || rs == config.session_id
                || !codex_provider_session_is_new(rs, &config.codex_cleared_provider_sessions)
        })
    {
        config.resume_session = None;
    }

    if config.resume_session.is_none() && config.provider == "codex" {
        if let Some((provider_session_id, _updated_at)) = manager::latest_codex_session_index_entry(
            &config.session_id,
        )?
        .filter(|(provider_session_id, _updated_at)| {
            codex_provider_session_is_new(
                provider_session_id,
                &config.codex_cleared_provider_sessions,
            )
        }) {
            config.resume_session = Some(provider_session_id);
            config.codex_cleared_provider_sessions.clear();
        }
    }

    if config.resume_session.is_none() {
        let should_fallback = match config.provider.as_str() {
            "opencode" => config.session_id.starts_with("ses_"),
            "codex" | "gemini" => false,
            _ => true,
        };
        if should_fallback {
            config.resume_session = Some(config.session_id.clone());
        }
    }

    Ok(())
}

fn prepare_clear_config(config: &mut AgentConfig) -> Result<(), String> {
    config.is_off = false;
    config.resume_session = None;
    config.fresh_provider_session_id = None;
    config.codex_cleared_provider_sessions.clear();
    if config.provider == "claude" {
        config.fresh_provider_session_id = Some(uuid::Uuid::new_v4().to_string());
    }
    Ok(())
}

fn codex_provider_session_is_new(candidate: &str, excluded: &[String]) -> bool {
    let candidate = candidate.trim();
    !candidate.is_empty()
        && !excluded
            .iter()
            .any(|session_id| session_id.trim() == candidate)
}

async fn is_name_unique(state: &AppState, name: &str, exclude_session_id: Option<&str>) -> bool {
    let agents = state.agents.lock().await;
    !agents.values().any(|a| {
        let config = a.config.lock().unwrap();
        config.session_name == name && exclude_session_id.is_none_or(|id| config.session_id != id)
    })
}

async fn is_session_id_available(state: &AppState, session_id: &str) -> bool {
    let agents = state.agents.lock().await;
    !agents.contains_key(session_id)
}

async fn register_new_agent(
    mut config: AgentConfig,
    mut actual_resume: Option<String>,
    state: &AppState,
    app: &AppHandle,
    clone_name_base: Option<&str>,
    reserved_session_name: Option<&str>,
    placement: AgentOrderPlacement<'_>,
) -> Result<AgentConfig, String> {
    let session_id = config.session_id.clone();
    config.system_include_directories = Some(crate::utils::fs::resolve_system_include_directories(
        &config.agent_class,
        &session_id,
    ));
    let mut active_agent = manager::spawn_agent(app.clone(), config.clone(), false, None).await?;
    // Propagate any fields that spawn_agent may have auto-assigned (e.g. opencode_port).
    if config.provider == "codex" && actual_resume.is_none() {
        for _ in 0..40 {
            let live_provider_session_id = active_agent
                .config
                .lock()
                .ok()
                .and_then(|cfg| cfg.resume_session.clone())
                .filter(|value| {
                    value != &session_id
                        && codex_provider_session_is_new(
                            value,
                            &config.codex_cleared_provider_sessions,
                        )
                });
            let provider_session_id = if live_provider_session_id.is_some() {
                live_provider_session_id
            } else {
                manager::latest_codex_session_index_entry(&session_id)?
                    .map(|(provider_session_id, _updated_at)| provider_session_id)
                    .filter(|value| {
                        codex_provider_session_is_new(
                            value,
                            &config.codex_cleared_provider_sessions,
                        )
                    })
            };

            if let Some(provider_session_id) = provider_session_id {
                actual_resume = Some(provider_session_id.clone());
                config.resume_session = Some(provider_session_id.clone());
                config.codex_cleared_provider_sessions.clear();
                {
                    let mut cfg = active_agent.config.lock().unwrap();
                    cfg.resume_session = Some(provider_session_id.clone());
                    cfg.codex_cleared_provider_sessions.clear();
                }
                manager::log_debug(&format!(
                    "[WARDIAN] Adopted live Codex session id {} for Wardian session {}",
                    provider_session_id, session_id
                ));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    let persisted_resume =
        persisted_resume_session_for_provider(&config.provider, actual_resume.clone(), &session_id);
    config.resume_session = persisted_resume.clone();

    {
        let mut cfg = active_agent.config.lock().unwrap();
        config.opencode_port = cfg.opencode_port;
        cfg.resume_session = persisted_resume;
    }

    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    if agents.contains_key(&session_id) {
        manager::terminate_active_agent_process(&mut active_agent);
        return Err(format!(
            "An agent with session ID '{}' already exists.",
            session_id
        ));
    }
    let existing_names = agents
        .values()
        .map(|agent| agent.config.lock().unwrap().session_name.clone())
        .collect::<std::collections::HashSet<_>>();
    match resolve_registered_session_name(&config.session_name, clone_name_base, &existing_names) {
        Ok(session_name) => config.session_name = session_name,
        Err(error) => {
            manager::terminate_active_agent_process(&mut active_agent);
            return Err(error);
        }
    }
    if let Some(reserved_session_name) = reserved_session_name {
        let mut reservations = state.agent_name_reservations.lock().await;
        reservations.remove(reserved_session_name);
    }
    {
        let mut cfg = active_agent.config.lock().unwrap();
        cfg.session_name = config.session_name.clone();
    }
    if let Some(ref tx) = active_agent.stdin_tx {
        if let Ok(mut senders) = state.input_senders.write() {
            senders.insert(session_id.clone(), tx.clone());
        }
    }
    agents.insert(session_id.clone(), active_agent);
    insert_new_agent_order(&mut order, &session_id, placement);
    manager::save_state(app, &agents, &order);
    let _ = app.emit("agents-updated", ());

    Ok(config)
}

#[tauri::command]
pub async fn spawn_agent(
    req: SpawnAgentRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AgentConfig, String> {
    let requested_session_name = req.session_name;
    let agent_class = req.agent_class;
    let folder = normalize_spawn_folder(&req.folder)?;
    let resume_session = req.resume_session;
    let is_off = req.is_off;
    let config_override = req.config_override;

    let name_reservation =
        reserve_spawn_session_name(&state, &requested_session_name, &agent_class).await?;
    let session_name = name_reservation.session_name.clone();

    manager::log_debug(&format!(
        "[WARDIAN] spawn_agent called for session name: {}, class: {}",
        session_name, agent_class
    ));
    let provider_name = config_override
        .as_ref()
        .map(|c| c.provider.clone())
        .unwrap_or_else(|| "claude".to_string());
    let mut actual_resume = resume_session.clone().filter(|s| !s.is_empty());

    let mut session_id = actual_resume.clone();

    if actual_resume.is_none() {
        if provider_uses_generated_session_id(&provider_name) {
            session_id = Some(uuid::Uuid::new_v4().to_string());
        } else {
            let cwd = crate::utils::fs::resolve_cwd(&folder, "");

            match manager::obtain_session_id(&cwd, Some(&agent_class), config_override.as_ref())
                .await
            {
                Ok(real_sid) => {
                    manager::log_debug(&format!(
                        "[WARDIAN] Intercepted stream-json session ID for {}: {}",
                        provider_name, real_sid
                    ));
                    // Properly set final_resume because manager::spawn_agent requires it to launch the persistent agent with --resume
                    session_id = Some(real_sid.clone());
                    actual_resume = Some(real_sid);
                }
                Err(e) => {
                    release_spawn_name_reservation(&state, &session_name).await;
                    return Err(format!("Failed to initialize the provider session: {}", e));
                }
            }
        }
    }

    let session_id = session_id.ok_or_else(|| "Failed to determine session ID".to_string())?;

    let mut config = config_override.unwrap_or_default();
    config.session_id = session_id.clone();
    config.session_name = session_name.clone();
    config.agent_class = agent_class.clone();
    config.folder = folder;
    config.resume_session = actual_resume.clone();
    config.is_off = is_off.unwrap_or(false);
    let registered = register_new_agent(
        config,
        actual_resume,
        &state,
        &app,
        None,
        Some(&session_name),
        AgentOrderPlacement::Top,
    )
    .await;
    if registered.is_err() {
        release_spawn_name_reservation(&state, &session_name).await;
    }
    registered
}

#[tauri::command]
pub async fn clone_agent(
    req: CloneAgentRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AgentConfig, String> {
    let source_session_id = req.source_session_id.trim().to_string();
    if source_session_id.is_empty() {
        return Err("Source agent is required.".to_string());
    }

    let (source_config, existing_names) = {
        let agents = state.agents.lock().await;
        let source = agents
            .get(&source_session_id)
            .ok_or_else(|| format!("Agent {} not found", source_session_id))?
            .config
            .lock()
            .unwrap()
            .clone();
        let names = agents
            .values()
            .map(|agent| agent.config.lock().unwrap().session_name.clone())
            .collect::<std::collections::HashSet<_>>();
        (source, names)
    };

    let requested_session_name = req.session_name.filter(|name| !name.trim().is_empty());
    let generated_session_name = requested_session_name.is_none();
    let session_name = requested_session_name
        .unwrap_or_else(|| clone_unique_name(&source_config.session_name, &existing_names));
    if !is_valid_name(&session_name) {
        return Err("Invalid agent name. Names must contain only alphanumeric characters, underscores, or hyphens (no spaces).".to_string());
    }
    if existing_names.contains(&session_name) {
        return Err(format!(
            "An agent with the name '{}' already exists.",
            session_name
        ));
    }

    manager::log_debug(&format!(
        "[WARDIAN] clone_agent called for source session: {}, mode: {:?}",
        source_session_id, req.mode
    ));

    let mut config = clone_sanitize_config(
        &source_config,
        session_name,
        req.provider,
        req.folder,
        req.agent_class,
        req.start.unwrap_or(true),
    );
    let provider_name = config.provider.clone();
    let mut actual_resume = None;
    let profile_home = if req.mode == CloneAgentMode::Profile {
        Some(
            crate::utils::fs::get_wardian_home()
                .ok_or_else(|| "Could not find Wardian home".to_string())?,
        )
    } else {
        None
    };
    let provisional_profile_session_id = (profile_home.is_some()
        && !provider_uses_generated_session_id(&provider_name))
    .then(|| uuid::Uuid::new_v4().to_string());

    let session_id = if provider_uses_generated_session_id(&provider_name) {
        let generated_session_id = uuid::Uuid::new_v4().to_string();
        config.session_id = generated_session_id.clone();
        if let Some(home) = profile_home.as_ref() {
            clone_ensure_profile_destination_available(home, &generated_session_id, None)?;
            clone_copy_agent_profile_files(home, &source_session_id, &generated_session_id)?;
        }
        generated_session_id
    } else {
        if let (Some(home), Some(provisional_session_id)) = (
            profile_home.as_ref(),
            provisional_profile_session_id.as_ref(),
        ) {
            config.session_id = provisional_session_id.clone();
            clone_ensure_profile_destination_available(home, provisional_session_id, None)?;
            clone_copy_agent_profile_files(home, &source_session_id, provisional_session_id)?;
            config.system_include_directories =
                Some(crate::utils::fs::resolve_system_include_directories(
                    &config.agent_class,
                    provisional_session_id,
                ));
        }
        let cwd = crate::utils::fs::resolve_cwd(&config.folder, "");
        let real_sid = manager::obtain_session_id(&cwd, Some(&config.agent_class), Some(&config))
            .await
            .map_err(|e| format!("Failed to initialize the provider session: {}", e))?;
        manager::log_debug(&format!(
            "[WARDIAN] Intercepted stream-json clone session ID for {}: {}",
            provider_name, real_sid
        ));
        actual_resume = Some(real_sid.clone());
        real_sid
    };

    config.session_id = session_id.clone();
    config.resume_session = actual_resume.clone();

    if !is_session_id_available(&state, &session_id).await {
        if let Some(home) = profile_home.as_ref() {
            if let Some(provisional_session_id) = provisional_profile_session_id.as_deref() {
                let provisional_root = home.join("agents").join(provisional_session_id);
                clone_remove_existing_path(&provisional_root);
            }
        }
        return Err(format!(
            "An agent with session ID '{}' already exists.",
            session_id
        ));
    }

    if let Some(home) = profile_home.as_ref() {
        let allowed_existing_profile_session_id =
            if provider_uses_generated_session_id(&provider_name) {
                Some(session_id.as_str())
            } else {
                provisional_profile_session_id.as_deref()
            };
        clone_ensure_profile_destination_available(
            home,
            &session_id,
            allowed_existing_profile_session_id,
        )?;
        clone_copy_agent_profile_files(home, &source_session_id, &session_id)?;
        if let Some(provisional_session_id) = provisional_profile_session_id
            .as_deref()
            .filter(|id| *id != session_id)
        {
            let provisional_root = home.join("agents").join(provisional_session_id);
            clone_remove_existing_path(&provisional_root);
        }
    }

    register_new_agent(
        config,
        actual_resume,
        &state,
        &app,
        generated_session_name.then_some(source_config.session_name.as_str()),
        None,
        AgentOrderPlacement::After(&source_session_id),
    )
    .await
}

#[tauri::command]
pub async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentConfig>, String> {
    manager::log_debug("[WARDIAN] list_agents called");
    let agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    let mut list: Vec<AgentConfig> = Vec::new();
    for id in order.iter() {
        if let Some(agent) = agents.get(id) {
            list.push(agent.config.lock().unwrap().clone());
        }
    }
    Ok(list)
}

#[tauri::command]
pub async fn list_agent_metrics(state: State<'_, AppState>) -> Result<Vec<AgentTelemetry>, String> {
    Ok(manager::get_all_metrics(&state).await)
}

#[tauri::command]
pub async fn kill_agent(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] kill_agent called for session: {}",
        session_id
    ));
    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    #[allow(unused_mut)]
    if let Some(mut agent) = agents.remove(&session_id) {
        // Remove from input_senders immediately
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        order.retain(|id| id != &session_id);
        manager::save_state(&app, &agents, &order);
        manager::terminate_active_agent_process(&mut agent);

        // Phase 2: Remove from SQLite
        let _ = wardian_core::db::delete_agent(&session_id);
        let _ = app.emit("agents-updated", ());

        // Cleanup: remove the agent's private directory
        if let Some(home) = crate::utils::fs::get_wardian_home() {
            let agent_dir = home.join("agents").join(&session_id);
            if agent_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&agent_dir) {
                    manager::log_debug(&format!(
                        "[WARDIAN] Failed to remove agent directory {:?}: {}",
                        agent_dir, e
                    ));
                } else {
                    manager::log_debug(&format!(
                        "[WARDIAN] Successfully removed agent directory {:?}",
                        agent_dir
                    ));
                }
            }
        }

        Ok(())
    } else {
        let err_msg = format!("Agent with session ID {} not found", session_id);
        manager::log_debug(&format!("[WARDIAN] {}", err_msg));
        Err(err_msg)
    }
}

#[tauri::command]
pub async fn pause_agent(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] pause_agent called for session: {}",
        session_id
    ));
    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;

    if let Some(agent) = agents.get_mut(&session_id) {
        manager::terminate_active_agent_process(agent);

        // For opencode: capture the real ses_xxx session ID from the log so
        // resume can pass --session ses_xxx rather than the internal UUID.
        capture_opencode_pause_resume_session(agent);
        mark_agent_paused_off(agent);

        // Remove from input_senders
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        manager::save_state(&app, &agents, &order);
        let _ = app.emit("agents-updated", ());
        let _ = app.emit(
            "agent-status-updated",
            agent_status_update_payload(&session_id, "Off"),
        );
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

#[tauri::command]
pub async fn resume_agent(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] resume_agent called for session: {}",
        session_id
    ));
    let snapshot = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        capture_resume_runtime_snapshot(agent)
    };

    let mut config = snapshot.config;
    prepare_resume_config(&mut config)?;
    let mut new_active = manager::spawn_agent(
        app.clone(),
        config.clone(),
        true,
        snapshot.init_timestamp.clone(),
    )
    .await?;
    restore_runtime_state_snapshot_after_resume(
        &mut new_active,
        snapshot.query_count,
        snapshot.init_timestamp,
        snapshot.log_path,
    );
    promote_fresh_provider_session_after_resume(&config.provider, &mut new_active);

    let stdin_tx = new_active.stdin_tx.clone();
    let mut old_agent = {
        let mut agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        let Some(old_agent) = agents.remove(&session_id) else {
            manager::terminate_active_agent_process(&mut new_active);
            return Err(format!("Agent {} not found", session_id));
        };
        agents.insert(session_id.clone(), new_active);
        manager::save_state(&app, &agents, &order);
        old_agent
    };

    sync_resumed_input_sender(&state, &session_id, stdin_tx);

    let _ = app.emit("agents-updated", ());
    let _ = app.emit(
        "agent-status-updated",
        agent_status_update_payload(&session_id, "Idle"),
    );
    manager::terminate_active_agent_process(&mut old_agent);
    Ok(())
}

#[tauri::command]
pub async fn clear_agent_session(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] clear_agent_session called for session: {}",
        session_id
    ));
    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;

    if let Some(agent) = agents.get_mut(&session_id) {
        // 1. Terminate the old agent's process tree immediately.
        manager::terminate_active_agent_process(agent);

        // 2. Prepare fresh config (new provider session ID for Claude, clear resume IDs)
        let mut config = agent.config.lock().unwrap().clone();
        let previous_codex_provider_sessions = if config.provider == "codex" {
            let mut sessions = config.codex_cleared_provider_sessions.clone();
            if let Some(session_id) = config
                .resume_session
                .clone()
                .filter(|session_id| !session_id.trim().is_empty())
            {
                if !sessions.iter().any(|existing| existing == &session_id) {
                    sessions.push(session_id);
                }
            }
            if let Some(session_id) = manager::latest_codex_session_index_entry(&config.session_id)
                .ok()
                .flatten()
                .map(|(provider_session_id, _updated_at)| provider_session_id)
                .filter(|session_id| !session_id.trim().is_empty())
            {
                if !sessions.iter().any(|existing| existing == &session_id) {
                    sessions.push(session_id);
                }
            }
            sessions
        } else {
            Vec::new()
        };
        prepare_clear_config(&mut config)?;
        if config.provider == "codex" {
            config.codex_cleared_provider_sessions = previous_codex_provider_sessions;
        }

        // For providers that bootstrap via obtain_session_id (e.g. Gemini), run the same
        // bootstrap after clear so the fresh session gets a trackable provider session ID.
        // Without this, resume_session stays None and log/status tracking breaks.
        if provider_needs_obtain_session_id_on_clear(&config.provider) {
            let cwd = crate::utils::fs::resolve_cwd(&config.folder, "");
            match manager::obtain_session_id(&cwd, Some(&config.agent_class), Some(&config)).await {
                Ok(new_session_id) if !new_session_id.is_empty() => {
                    manager::log_debug(&format!(
                        "[WARDIAN] clear_agent_session: obtained fresh {} session ID: {}",
                        config.provider, new_session_id
                    ));
                    config.resume_session = Some(new_session_id);
                }
                Ok(_) => {
                    manager::log_debug(&format!(
                        "[WARDIAN] clear_agent_session: obtain_session_id returned empty ID for {}",
                        config.provider
                    ));
                }
                Err(e) => {
                    manager::log_debug(&format!(
                        "[WARDIAN] clear_agent_session: failed to obtain fresh {} session ID: {}",
                        config.provider, e
                    ));
                }
            }
        }

        // 3. Reset UI and in-memory buffers
        if let Ok(mut buf) = agent.output_buffer.lock() {
            buf.clear();
        }
        if let Ok(mut title) = agent.terminal_title.lock() {
            title.clear();
        }
        if let Ok(mut status) = agent.current_status.lock() {
            *status = "Processing...".to_string();
        }
        if let Ok(mut count) = agent.query_count.lock() {
            *count = 0;
        }
        if let Ok(mut log_path) = agent.log_path.lock() {
            *log_path = None;
        }
        if let Ok(mut log_last_modified) = agent.log_last_modified.lock() {
            *log_last_modified = None;
        }

        let _ = app.emit(
            "agent-terminal-cleared",
            serde_json::json!({ "session_id": session_id }),
        );

        // 5. Spawn a FRESH process (is_restored = false)
        // This ensures Claude uses --session-id and others start clean.
        let born = agent.init_timestamp.lock().unwrap().clone();
        let new_active = manager::spawn_agent(app.clone(), config, false, born).await?;

        {
            let mut new_config = new_active.config.lock().unwrap();
            let mut old_config = agent.config.lock().unwrap();
            if old_config.provider == "claude" {
                if let Some(fresh_provider_session_id) = new_config.fresh_provider_session_id.take()
                {
                    new_config.resume_session = Some(fresh_provider_session_id);
                }
            }
            *old_config = new_config.clone();
        }

        // 6. Register new input sender
        if let Some(ref tx) = new_active.stdin_tx {
            if let Ok(mut senders) = state.input_senders.write() {
                senders.insert(session_id.clone(), tx.clone());
            }
        }

        // 7. Update agent metadata in SQLite
        {
            let config = agent.config.lock().unwrap();
            let born = agent.init_timestamp.lock().unwrap();
            let workspace = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id)
                .to_string_lossy()
                .to_string();
            let project = wardian_core::db::project_name_from_workspace(&workspace);
            let _ = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
                session_id: &config.session_id,
                session_name: &config.session_name,
                agent_class: &config.agent_class,
                provider: &config.provider,
                workspace: Some(&workspace),
                project: project.as_deref(),
                is_off: config.is_off,
                created_at: born.as_deref(),
            });
        }

        // 8. Swap the struct
        let _ = std::mem::replace(agent, new_active);
        manager::save_state(&app, &agents, &order);

        // 9. Force a frontend refresh and terminal resize to clear glitches
        let _ = app.emit("agents-updated", ());
        let _ = app.emit(
            "agent-terminal-resize",
            serde_json::json!({ "session_id": session_id, "rows": 24, "cols": 80 }),
        );
        let _ = app.emit(
            "agent-pty-output-ready",
            serde_json::json!({ "session_id": session_id }),
        );
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

#[tauri::command]
pub async fn rename_agent(
    session_id: String,
    new_name: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] rename_agent called for session {}: {}",
        session_id, new_name
    ));

    if !is_valid_name(&new_name) {
        return Err("Invalid agent name. Names must contain only alphanumeric characters, underscores, or hyphens (no spaces).".to_string());
    }

    if !is_name_unique(&state, &new_name, Some(&session_id)).await {
        return Err(format!(
            "An agent with the name '{}' already exists.",
            new_name
        ));
    }

    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;

    if let Some(agent) = agents.get_mut(&session_id) {
        let (sid, name, class, provider, workspace, is_off, born) = {
            let mut config = agent.config.lock().unwrap();
            config.session_name = new_name;
            let workspace = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id)
                .to_string_lossy()
                .to_string();
            (
                config.session_id.clone(),
                config.session_name.clone(),
                config.agent_class.clone(),
                config.provider.clone(),
                workspace,
                config.is_off,
                agent.init_timestamp.lock().unwrap().clone(),
            )
        };

        // Phase 2: Update agent metadata in SQLite
        let project = wardian_core::db::project_name_from_workspace(&workspace);
        let _ = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: &sid,
            session_name: &name,
            agent_class: &class,
            provider: &provider,
            workspace: Some(&workspace),
            project: project.as_deref(),
            is_off,
            created_at: born.as_deref(),
        });
        manager::save_state(&app, &agents, &order);
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

#[tauri::command]
pub async fn update_agent_config(
    mut new_config: AgentConfig,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] update_agent_config called for session: {}",
        new_config.session_id
    ));
    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;

    if let Some(agent) = agents.get_mut(&new_config.session_id) {
        // If class has changed, auto-update the system_include_directories
        let current_class = {
            let config = agent.config.lock().unwrap();
            config.agent_class.clone()
        };

        if current_class != new_config.agent_class {
            manager::log_debug(&format!(
                "[WARDIAN] Agent class changed from {} to {}. Updating system include directories.",
                current_class, new_config.agent_class
            ));
            new_config.system_include_directories =
                Some(crate::utils::fs::resolve_system_include_directories(
                    &new_config.agent_class,
                    &new_config.session_id,
                ));
        }

        {
            let mut config = agent.config.lock().unwrap();
            *config = new_config;
        }
        manager::save_state(&app, &agents, &order);
        Ok(())
    } else {
        Err(format!("Agent {} not found", new_config.session_id))
    }
}

#[tauri::command]
pub async fn reorder_agents(
    session_ids: Vec<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug("[WARDIAN] reorder_agents called");
    let agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    *order = session_ids;
    manager::save_state(&app, &agents, &order);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        agent_status_update_payload, build_agent_clone_preview, capture_opencode_pause_resume_session,
        capture_resume_runtime_snapshot, clone_collect_eligible_file_tree,
        clone_copy_agent_profile_files, clone_ensure_profile_destination_available,
        clone_sanitize_config, clone_unique_name, codex_provider_session_is_new,
        flatten_clone_file_paths, generated_agent_name, insert_new_agent_order,
        mark_agent_paused_off, normalize_spawn_folder, persisted_resume_session_for_provider,
        prepare_clear_config, prepare_resume_config, promote_fresh_provider_session_after_resume,
        provider_needs_obtain_session_id_on_clear, provider_uses_generated_session_id,
        reserve_spawn_session_name, resolve_requested_spawn_session_name,
        restore_runtime_state_snapshot_after_resume, sync_resumed_input_sender, AgentOrderPlacement,
    };
    use crate::state::{ActiveAgent, AppState};
    use crate::utils::fs::create_directory_link;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use wardian_core::models::{AgentConfig, AgentSessionPersistenceOverride, DeployedSkillRef};

    struct WardianHomeGuard;

    impl Drop for WardianHomeGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("WARDIAN_HOME") };
        }
    }

    fn make_test_agent() -> ActiveAgent {
        ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig::default())),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: Arc::new(Mutex::new(String::new())),
            process_id: None,
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Idle".to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
                "test-agent".to_string(),
                4096,
                262_144,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    fn use_isolated_resume_setting() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: wardian_core::models::AgentSessionPersistence::Resume,
            ..Default::default()
        })
        .expect("save shell settings");
        (guard, temp)
    }

    fn clone_name_set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn clone_preview_defaults_to_profile_selection() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source_root = home.join("agents").join("source-agent");
        std::fs::create_dir_all(&source_root).expect("source root");
        std::fs::write(source_root.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(source_root.join("notes.md"), "notes").expect("notes");

        let existing_names = HashSet::new();
        let preview = build_agent_clone_preview(
            home,
            "source-agent",
            &AgentConfig {
                session_id: "source-agent".to_string(),
                session_name: "Alpha".to_string(),
                agent_class: "Coder".to_string(),
                folder: "D:/Development/Wardian".to_string(),
                provider: "codex".to_string(),
                ..Default::default()
            },
            &existing_names,
        )
        .expect("preview");

        assert_eq!(preview.suggested_session_name, "Alpha-copy");
        assert_eq!(preview.default_selected_files, vec!["AGENTS.md".to_string()]);
        assert!(preview
            .files
            .children
            .iter()
            .any(|node| node.path() == "notes.md"));
    }

    #[test]
    fn clone_preview_includes_agent_specific_skills_as_default_selected() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;

        let source_root = temp.path().join("agents").join("source-agent");
        std::fs::create_dir_all(&source_root).expect("source root");
        std::fs::write(source_root.join("AGENTS.md"), "# Agent\n").expect("agents");
        let library_skill = temp
            .path()
            .join("library")
            .join("skills")
            .join("group-a")
            .join("planner");
        std::fs::create_dir_all(&library_skill).expect("library skill");
        std::fs::write(library_skill.join("SKILL.md"), "planner").expect("skill");
        crate::commands::library::deploy_skill_from_library(
            "group-a/planner",
            "agent",
            "source-agent",
        )
        .expect("deploy");

        let preview = build_agent_clone_preview(
            temp.path(),
            "source-agent",
            &AgentConfig {
                session_id: "source-agent".to_string(),
                session_name: "Alpha".to_string(),
                agent_class: "Coder".to_string(),
                folder: "D:/Development/Wardian".to_string(),
                provider: "codex".to_string(),
                ..Default::default()
            },
            &HashSet::new(),
        )
        .expect("preview");

        let expected = DeployedSkillRef {
            name: "planner".to_string(),
            source_path: Some("group-a/planner".to_string()),
        };
        assert_eq!(preview.skills, vec![expected.clone()]);
        assert_eq!(preview.default_selected_skills, vec![expected]);
    }

    #[test]
    fn clone_preview_excludes_runtime_generated_and_skill_paths() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source_root = home.join("agents").join("source-agent");
        std::fs::create_dir_all(source_root.join("nested")).expect("nested");
        std::fs::create_dir_all(source_root.join("habitat")).expect("habitat");
        std::fs::create_dir_all(source_root.join(".agents").join("skills").join("planner"))
            .expect("skills");
        std::fs::create_dir_all(source_root.join(".codex")).expect("codex");
        std::fs::write(source_root.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(source_root.join("nested").join("keep.md"), "keep").expect("keep");
        std::fs::write(source_root.join("habitat").join("AGENTS.md"), "generated")
            .expect("generated");
        std::fs::write(
            source_root
                .join(".agents")
                .join("skills")
                .join("planner")
                .join("SKILL.md"),
            "skill",
        )
        .expect("skill");
        std::fs::write(source_root.join(".codex").join("history.jsonl"), "{}").expect("history");

        let files = clone_collect_eligible_file_tree(home, "source-agent").expect("files");
        let paths = flatten_clone_file_paths(&files);

        assert!(paths.contains(&"AGENTS.md".to_string()));
        assert!(paths.contains(&"nested/keep.md".to_string()));
        assert!(!paths.iter().any(|path| path.starts_with("habitat/")));
        assert!(!paths
            .iter()
            .any(|path| path.starts_with(".agents/skills/")));
        assert!(!paths.iter().any(|path| path.starts_with(".codex/")));
    }

    #[test]
    fn clone_preview_omits_directory_links() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source_root = home.join("agents").join("source-agent");
        let external = home.join("external");
        std::fs::create_dir_all(&source_root).expect("source root");
        std::fs::create_dir_all(&external).expect("external");
        std::fs::write(source_root.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(external.join("secret.md"), "secret").expect("secret");

        let linked = source_root.join("linked");
        if create_directory_link(&external, &linked).is_err() {
            return;
        }

        let files = clone_collect_eligible_file_tree(home, "source-agent").expect("files");
        let paths = flatten_clone_file_paths(&files);

        assert!(!paths.iter().any(|path| path.starts_with("linked/")));
    }

    #[test]
    fn fresh_spawn_order_inserts_new_agent_at_top() {
        let mut order = vec!["alpha".to_string(), "beta".to_string()];

        insert_new_agent_order(&mut order, "gamma", AgentOrderPlacement::Top);

        assert_eq!(order, vec!["gamma", "alpha", "beta"]);
    }

    #[test]
    fn clone_order_inserts_new_agent_after_source() {
        let mut order = vec!["alpha".to_string(), "beta".to_string()];

        insert_new_agent_order(
            &mut order,
            "alpha-copy",
            AgentOrderPlacement::After("alpha"),
        );

        assert_eq!(order, vec!["alpha", "alpha-copy", "beta"]);
    }

    #[test]
    fn clone_order_falls_back_to_top_when_source_is_missing() {
        let mut order = vec!["alpha".to_string(), "beta".to_string()];

        insert_new_agent_order(
            &mut order,
            "orphan-copy",
            AgentOrderPlacement::After("missing"),
        );

        assert_eq!(order, vec!["orphan-copy", "alpha", "beta"]);
    }

    #[test]
    fn new_agent_order_removes_duplicate_session_before_inserting() {
        let mut order = vec!["alpha".to_string(), "beta".to_string(), "alpha".to_string()];

        insert_new_agent_order(&mut order, "alpha", AgentOrderPlacement::Top);

        assert_eq!(order, vec!["alpha", "beta"]);
    }

    #[test]
    fn clone_unique_name_uses_copy_suffix_when_available() {
        assert_eq!(
            clone_unique_name("Alpha", &clone_name_set(&["Alpha"])),
            "Alpha-copy"
        );
    }

    #[test]
    fn clone_unique_name_increments_copy_suffix_until_available() {
        assert_eq!(
            clone_unique_name("Alpha", &clone_name_set(&["Alpha", "Alpha-copy"])),
            "Alpha-copy-2"
        );
    }

    #[test]
    fn generated_agent_name_uses_class_and_lowest_available_suffix() {
        assert_eq!(
            generated_agent_name("Coder", &clone_name_set(&["Coder-1", "Coder-2"])),
            "Coder-3"
        );
    }

    #[test]
    fn generated_agent_name_sanitizes_class_name() {
        assert_eq!(
            generated_agent_name("Data Analyst", &HashSet::new()),
            "Data-Analyst-1"
        );
    }

    #[test]
    fn generated_agent_name_falls_back_when_class_has_no_valid_name_chars() {
        assert_eq!(generated_agent_name(" !!! ", &HashSet::new()), "Agent-1");
    }

    #[test]
    fn explicit_spawn_name_with_spaces_still_fails_validation() {
        let err = resolve_requested_spawn_session_name(" Coder ", "Coder", &HashSet::new())
            .expect_err("explicit names with spaces must remain invalid");

        assert!(err.contains("Invalid agent name"));
    }

    #[tokio::test]
    async fn blank_spawn_name_reservations_choose_unique_names_before_spawn() {
        let state = AppState::new();

        let first = reserve_spawn_session_name(&state, "", "Coder")
            .await
            .expect("first reservation");
        let second = reserve_spawn_session_name(&state, "", "Coder")
            .await
            .expect("second reservation");

        assert_eq!(first.session_name, "Coder-1");
        assert_eq!(second.session_name, "Coder-2");
    }

    #[test]
    fn normalize_spawn_folder_stores_forward_slash_absolute_paths() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let slash_input = workspace.to_string_lossy().replace('\\', "/");
        let normalized = normalize_spawn_folder(&slash_input).expect("normalize slash path");

        assert!(!normalized.contains('\\'));
        assert!(std::path::Path::new(&normalized).is_absolute());
    }

    #[cfg(windows)]
    #[test]
    fn normalize_spawn_folder_accepts_windows_separator_variants() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let slash_input = workspace.to_string_lossy().replace('\\', "/");
        let backslash_input = slash_input.replace('/', "\\");
        let doubled_backslash_input = slash_input.replace('/', "\\\\");

        let slash_normalized = normalize_spawn_folder(&slash_input).expect("slash path");
        let backslash_normalized =
            normalize_spawn_folder(&backslash_input).expect("backslash path");
        let doubled_backslash_normalized =
            normalize_spawn_folder(&doubled_backslash_input).expect("doubled backslash path");

        assert_eq!(
            slash_normalized, backslash_normalized,
            "single backslash path should normalize the same as slash path"
        );
        assert_eq!(
            slash_normalized, doubled_backslash_normalized,
            "doubled backslash path should normalize the same as slash path"
        );
    }

    #[test]
    fn clone_sanitize_config_preserves_visible_setup_and_clears_runtime_memory() {
        let source = AgentConfig {
            session_id: "source-session".to_string(),
            session_name: "Alpha".to_string(),
            agent_class: "Coder".to_string(),
            folder: "D:/Development/Wardian".to_string(),
            provider: "codex".to_string(),
            model: Some("gpt-5.4".to_string()),
            resume_session: Some("provider-session".to_string()),
            git_worktree: Some(true),
            fresh_provider_session_id: Some("fresh-provider-session".to_string()),
            codex_cleared_provider_sessions: vec!["old-provider-session".to_string()],
            is_off: true,
            custom_args: Some("--verbose".to_string()),
            ..Default::default()
        };

        let clone =
            clone_sanitize_config(&source, "Alpha-copy".to_string(), None, None, None, true);

        assert_eq!(clone.session_id, "");
        assert_eq!(clone.session_name, "Alpha-copy");
        assert_eq!(clone.agent_class, "Coder");
        assert_eq!(clone.folder, "D:/Development/Wardian");
        assert_eq!(clone.provider, "codex");
        assert_eq!(clone.git_worktree, Some(true));
        assert_eq!(clone.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(clone.custom_args.as_deref(), Some("--verbose"));
        assert_eq!(clone.resume_session, None);
        assert_eq!(clone.fresh_provider_session_id, None);
        assert!(clone.codex_cleared_provider_sessions.is_empty());
        assert!(!clone.is_off);
    }

    #[test]
    fn clone_sanitize_config_strips_custom_provider_session_args() {
        let source = AgentConfig {
            custom_args: Some(
                "--verbose --resume old-gemini --session ses_old --session-id old-claude -r old-short --continue -c resume old-codex --model 'kept model'".to_string(),
            ),
            ..Default::default()
        };

        let clone =
            clone_sanitize_config(&source, "Alpha-copy".to_string(), None, None, None, true);
        let custom_args = clone.custom_args.expect("custom args");
        let parsed = shlex::split(&custom_args).expect("parse sanitized custom args");

        assert_eq!(
            parsed,
            vec![
                "--verbose".to_string(),
                "--model".to_string(),
                "kept model".to_string()
            ]
        );
    }

    #[test]
    fn clone_sanitize_config_strips_equals_form_session_args() {
        let source = AgentConfig {
            custom_args: Some(
                "--resume=old --session=ses_old --session-id=old -r=old --safe".to_string(),
            ),
            ..Default::default()
        };

        let clone =
            clone_sanitize_config(&source, "Alpha-copy".to_string(), None, None, None, true);

        assert_eq!(clone.custom_args.as_deref(), Some("--safe"));
    }

    #[test]
    fn clone_sanitize_config_applies_custom_overrides() {
        let source = AgentConfig {
            session_name: "Alpha".to_string(),
            agent_class: "Coder".to_string(),
            folder: "D:/source".to_string(),
            provider: "claude".to_string(),
            ..Default::default()
        };

        let clone = clone_sanitize_config(
            &source,
            "Beta".to_string(),
            Some("gemini".to_string()),
            Some("D:/target".to_string()),
            Some("Reviewer".to_string()),
            false,
        );

        assert_eq!(clone.session_name, "Beta");
        assert_eq!(clone.provider, "gemini");
        assert_eq!(clone.folder, "D:/target");
        assert_eq!(clone.agent_class, "Reviewer");
        assert!(clone.is_off);
    }

    #[test]
    fn clone_profile_copy_carries_whitelisted_files_and_excludes_runtime_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        let dest = home.join("agents").join("dest-agent");
        std::fs::create_dir_all(source.join(".agents").join("skills").join("skill-one"))
            .expect("create source skill");
        std::fs::create_dir_all(source.join("habitat")).expect("create habitat");
        std::fs::create_dir_all(source.join("claude")).expect("create claude dir");
        std::fs::write(source.join("AGENTS.md"), "# Agent Memory\n").expect("write agents");
        std::fs::write(
            source
                .join(".agents")
                .join("skills")
                .join("skill-one")
                .join("SKILL.md"),
            "# Skill\n",
        )
        .expect("write skill");
        std::fs::write(source.join("habitat").join("AGENTS.md"), "generated")
            .expect("write habitat");
        std::fs::write(
            source.join("claude").join("permission-requests.jsonl"),
            "{}\n",
        )
        .expect("write log");

        clone_copy_agent_profile_files(home, "source-agent", "dest-agent")
            .expect("copy profile files");

        assert_eq!(
            std::fs::read_to_string(dest.join("AGENTS.md")).expect("read copied agents"),
            "# Agent Memory\n"
        );
        assert_eq!(
            std::fs::read_to_string(
                dest.join(".agents")
                    .join("skills")
                    .join("skill-one")
                    .join("SKILL.md")
            )
            .expect("read copied skill"),
            "# Skill\n"
        );
        assert!(!dest.join("habitat").exists());
        assert!(!dest
            .join("claude")
            .join("permission-requests.jsonl")
            .exists());
    }

    #[test]
    fn clone_profile_destination_rejects_existing_agent_directory() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let destination = home.join("agents").join("existing-agent");
        std::fs::create_dir_all(&destination).expect("create destination");
        std::fs::write(destination.join("AGENTS.md"), "existing").expect("write existing profile");

        let err = clone_ensure_profile_destination_available(home, "existing-agent", None)
            .expect_err("existing destination should be rejected");

        assert!(err.contains("Cannot clone profile into existing agent directory"));
        assert_eq!(
            std::fs::read_to_string(destination.join("AGENTS.md")).expect("read existing profile"),
            "existing"
        );
    }

    #[test]
    fn clone_profile_destination_allows_provisional_directory() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        std::fs::create_dir_all(home.join("agents").join("provisional-agent"))
            .expect("create provisional destination");

        clone_ensure_profile_destination_available(
            home,
            "provisional-agent",
            Some("provisional-agent"),
        )
        .expect("matching provisional destination is allowed");
    }

    #[test]
    fn clone_profile_copy_carries_linked_skills() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        let dest = home.join("agents").join("dest-agent");
        let library_skill = home.join("library").join("skills").join("linked-skill");
        let source_skills = source.join(".agents").join("skills");
        let source_skill_link = source_skills.join("linked-skill");

        std::fs::create_dir_all(&library_skill).expect("create library skill");
        std::fs::create_dir_all(&source_skills).expect("create source skills");
        std::fs::write(library_skill.join("SKILL.md"), "# Linked Skill\n")
            .expect("write linked skill");
        create_directory_link(&library_skill, &source_skill_link).expect("link source skill");

        clone_copy_agent_profile_files(home, "source-agent", "dest-agent")
            .expect("copy profile files");

        assert_eq!(
            std::fs::read_to_string(
                dest.join(".agents")
                    .join("skills")
                    .join("linked-skill")
                    .join("SKILL.md")
            )
            .expect("read copied linked skill"),
            "# Linked Skill\n"
        );
        assert!(
            std::fs::read_link(dest.join(".agents").join("skills").join("linked-skill")).is_ok()
        );
    }

    #[test]
    fn opencode_uses_provider_session_id_instead_of_generated_uuid() {
        assert!(!provider_uses_generated_session_id("opencode"));
    }

    #[test]
    fn claude_keeps_generated_session_ids() {
        assert!(provider_uses_generated_session_id("claude"));
        assert!(provider_uses_generated_session_id("codex"));
    }

    #[test]
    fn mock_uses_generated_session_id() {
        assert!(provider_uses_generated_session_id("mock"));
    }

    #[test]
    fn gemini_needs_obtain_session_id_on_clear() {
        assert!(provider_needs_obtain_session_id_on_clear("gemini"));
        assert!(!provider_needs_obtain_session_id_on_clear("claude"));
        assert!(!provider_needs_obtain_session_id_on_clear("codex"));
        assert!(!provider_needs_obtain_session_id_on_clear("opencode"));
    }

    #[test]
    fn resume_snapshot_restores_query_count_and_log_path() {
        let mut new_active = make_test_agent();
        restore_runtime_state_snapshot_after_resume(
            &mut new_active,
            3,
            Some("2026-04-12T17:00:00.000Z".to_string()),
            Some(std::path::PathBuf::from("C:/tmp/session.json")),
        );

        assert_eq!(*new_active.query_count.lock().unwrap(), 3);
        assert_eq!(
            new_active.init_timestamp.lock().unwrap().as_deref(),
            Some("2026-04-12T17:00:00.000Z")
        );
        assert_eq!(
            new_active.log_path.lock().unwrap().as_deref(),
            Some(std::path::Path::new("C:/tmp/session.json"))
        );
    }

    #[test]
    fn claude_resume_promotes_fresh_provider_session_to_resume_session() {
        let mut new_active = make_test_agent();
        {
            let mut config = new_active.config.lock().unwrap();
            config.fresh_provider_session_id = Some("claude-fresh-session".to_string());
            config.resume_session = None;
        }

        promote_fresh_provider_session_after_resume("claude", &mut new_active);

        let config = new_active.config.lock().unwrap();
        assert_eq!(
            config.resume_session.as_deref(),
            Some("claude-fresh-session")
        );
        assert_eq!(config.fresh_provider_session_id, None);
    }

    #[test]
    fn non_claude_resume_keeps_fresh_provider_session_field() {
        let mut new_active = make_test_agent();
        {
            let mut config = new_active.config.lock().unwrap();
            config.fresh_provider_session_id = Some("provider-session".to_string());
            config.resume_session = None;
        }

        promote_fresh_provider_session_after_resume("codex", &mut new_active);

        let config = new_active.config.lock().unwrap();
        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.fresh_provider_session_id.as_deref(),
            Some("provider-session")
        );
    }

    #[test]
    fn sync_resumed_input_sender_inserts_or_removes_sender() {
        let state = AppState::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);

        sync_resumed_input_sender(&state, "agent-1", Some(tx));
        assert!(state.input_senders.read().unwrap().contains_key("agent-1"));

        sync_resumed_input_sender(&state, "agent-1", None);
        assert!(!state.input_senders.read().unwrap().contains_key("agent-1"));
    }

    #[test]
    fn agent_status_update_payload_uses_frontend_status_contract() {
        assert_eq!(
            agent_status_update_payload("agent-1", "Idle"),
            serde_json::json!({
                "session_id": "agent-1",
                "current_status": "Idle",
            })
        );
    }

    #[test]
    fn capture_resume_runtime_snapshot_reads_resume_fields_without_holding_state_locks() {
        let active = make_test_agent();
        {
            let mut config = active.config.lock().unwrap();
            config.session_id = "agent-1".to_string();
            config.provider = "codex".to_string();
        }
        *active.init_timestamp.lock().unwrap() = Some("2026-05-07T00:00:00Z".to_string());
        *active.query_count.lock().unwrap() = 7;
        *active.log_path.lock().unwrap() = Some(std::path::PathBuf::from("D:/tmp/agent.log"));

        let snapshot = capture_resume_runtime_snapshot(&active);

        assert_eq!(snapshot.config.session_id, "agent-1");
        assert_eq!(snapshot.config.provider, "codex");
        assert_eq!(
            snapshot.init_timestamp.as_deref(),
            Some("2026-05-07T00:00:00Z")
        );
        assert_eq!(snapshot.query_count, 7);
        assert_eq!(
            snapshot.log_path.as_deref(),
            Some(std::path::Path::new("D:/tmp/agent.log"))
        );
    }

    #[test]
    fn mark_agent_paused_off_clears_runtime_channels_and_status() {
        let mut active = make_test_agent();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        active.stdin_tx = Some(tx);
        {
            let mut config = active.config.lock().unwrap();
            config.is_off = false;
        }

        mark_agent_paused_off(&mut active);

        assert!(active.stdin_tx.is_none());
        assert!(active.pty_master.is_none());
        assert!(active.config.lock().unwrap().is_off);
        assert_eq!(active.current_status.lock().unwrap().as_str(), "Off");
    }

    #[test]
    fn opencode_pause_captures_real_session_id_from_log() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("opencode.log");
        std::fs::write(
            &log_path,
            "INFO service=session id=ses_old created\nINFO service=session id=ses_new created\n",
        )
        .expect("write opencode log");
        let active = make_test_agent();
        {
            let mut config = active.config.lock().unwrap();
            config.provider = "opencode".to_string();
            config.resume_session = Some("stale-uuid".to_string());
        }
        *active.log_path.lock().unwrap() = Some(log_path);

        capture_opencode_pause_resume_session(&active);

        assert_eq!(
            active.config.lock().unwrap().resume_session.as_deref(),
            Some("ses_new")
        );
    }

    #[test]
    fn opencode_pause_keeps_existing_real_session_id() {
        let active = make_test_agent();
        {
            let mut config = active.config.lock().unwrap();
            config.provider = "opencode".to_string();
            config.resume_session = Some("ses_existing".to_string());
        }

        capture_opencode_pause_resume_session(&active);

        assert_eq!(
            active.config.lock().unwrap().resume_session.as_deref(),
            Some("ses_existing")
        );
    }

    #[test]
    fn claude_persists_resume_session_after_initial_spawn() {
        assert_eq!(
            persisted_resume_session_for_provider("claude", None, "claude-session-1"),
            Some("claude-session-1".to_string())
        );
    }

    #[test]
    fn non_claude_providers_leave_resume_session_unchanged() {
        assert_eq!(
            persisted_resume_session_for_provider("codex", None, "codex-session-1"),
            None
        );
    }

    #[test]
    fn opencode_resume_sets_resume_session_and_clears_off() {
        let (_guard, _temp) = use_isolated_resume_setting();
        // session_id already in ses_xxx format → resume_session inherits it
        let mut config = AgentConfig {
            provider: "opencode".to_string(),
            session_id: "ses_test".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session.as_deref(), Some("ses_test"));
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn opencode_resume_with_uuid_session_id_leaves_resume_session_none() {
        let (_guard, _temp) = use_isolated_resume_setting();
        // session_id is a Wardian UUID (not ses_xxx) → must NOT be used as --session
        let mut config = AgentConfig {
            provider: "opencode".to_string(),
            session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn opencode_resume_clears_stale_uuid_resume_session() {
        let (_guard, _temp) = use_isolated_resume_setting();
        // Stale state: resume_session was previously saved as a UUID (pre-fix)
        let mut config = AgentConfig {
            provider: "opencode".to_string(),
            session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            resume_session: Some("550e8400-e29b-41d4-a716-446655440000".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn non_opencode_resume_clears_off_and_sets_resume_session() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "claude-session".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session.as_deref(), Some("claude-session"));
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_resume_without_provider_thread_id_does_not_use_wardian_uuid() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: "22ff532b-007a-44c9-a4b4-9b7c0f546274".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_resume_clears_stale_wardian_uuid_resume_session() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: "22ff532b-007a-44c9-a4b4-9b7c0f546274".to_string(),
            resume_session: Some("22ff532b-007a-44c9-a4b4-9b7c0f546274".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_resume_recovers_provider_thread_id_from_projected_history() {
        let (_guard, temp) = use_isolated_resume_setting();
        let session_id = "22ff532b-007a-44c9-a4b4-9b7c0f546274";
        let codex_home = temp
            .path()
            .join("agents")
            .join(session_id)
            .join("habitat")
            .join(".codex");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(
            codex_home.join("history.jsonl"),
            "{\"session_id\":\"019db2f3-22de-7861-8bc6-1b86db1686db\",\"ts\":1776823781,\"text\":\"Hello\"}\n",
        )
        .expect("write history");
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: session_id.to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(
            config.resume_session.as_deref(),
            Some("019db2f3-22de-7861-8bc6-1b86db1686db")
        );
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_resume_with_pending_clear_does_not_reuse_excluded_provider_thread() {
        let (_guard, temp) = use_isolated_resume_setting();
        let session_id = "22ff532b-007a-44c9-a4b4-9b7c0f546274";
        let old_provider_session_id = "019db2f3-22de-7861-8bc6-1b86db1686db";
        let codex_home = temp
            .path()
            .join("agents")
            .join(session_id)
            .join("habitat")
            .join(".codex");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(
            codex_home.join("history.jsonl"),
            format!(
                "{{\"session_id\":\"{}\",\"ts\":1776823781,\"text\":\"Before clear\"}}\n",
                old_provider_session_id
            ),
        )
        .expect("write history");
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: session_id.to_string(),
            resume_session: None,
            codex_cleared_provider_sessions: vec![old_provider_session_id.to_string()],
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.codex_cleared_provider_sessions,
            vec![old_provider_session_id.to_string()]
        );
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_resume_with_pending_clear_adopts_new_provider_thread() {
        let (_guard, temp) = use_isolated_resume_setting();
        let session_id = "22ff532b-007a-44c9-a4b4-9b7c0f546274";
        let old_provider_session_id = "019db2f3-22de-7861-8bc6-1b86db1686db";
        let new_provider_session_id = "019db30f-12fc-7ef0-8faa-8d88703dc124";
        let codex_home = temp
            .path()
            .join("agents")
            .join(session_id)
            .join("habitat")
            .join(".codex");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(
            codex_home.join("history.jsonl"),
            format!(
                "{{\"session_id\":\"{}\",\"ts\":1776823781,\"text\":\"Before clear\"}}\n{{\"session_id\":\"{}\",\"ts\":1776823881,\"text\":\"After clear\"}}\n",
                old_provider_session_id, new_provider_session_id
            ),
        )
        .expect("write history");
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: session_id.to_string(),
            resume_session: None,
            codex_cleared_provider_sessions: vec![old_provider_session_id.to_string()],
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(
            config.resume_session.as_deref(),
            Some(new_provider_session_id)
        );
        assert!(config.codex_cleared_provider_sessions.is_empty());
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_clear_session_candidates_exclude_previous_provider_thread() {
        let excluded = vec!["019db2f3-22de-7861-8bc6-1b86db1686db".to_string()];

        assert!(!codex_provider_session_is_new("", &excluded));
        assert!(!codex_provider_session_is_new(
            "019db2f3-22de-7861-8bc6-1b86db1686db",
            &excluded
        ));
        assert!(codex_provider_session_is_new(
            "019db30f-12fc-7ef0-8faa-8d88703dc124",
            &excluded
        ));
    }

    #[test]
    fn global_fresh_session_persistence_resume_clears_resume_session() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: wardian_core::models::AgentSessionPersistence::Fresh,
            ..Default::default()
        })
        .expect("save shell settings");

        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: Some("provider-session".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn agent_resume_override_fresh_wins_over_global_resume() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: wardian_core::models::AgentSessionPersistence::Resume,
            ..Default::default()
        })
        .expect("save shell settings");

        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: Some("provider-session".to_string()),
            session_persistence: AgentSessionPersistenceOverride::Fresh,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn agent_resume_override_resume_wins_over_global_fresh() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: wardian_core::models::AgentSessionPersistence::Fresh,
            ..Default::default()
        })
        .expect("save shell settings");

        let mut config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "claude-session".to_string(),
            resume_session: None,
            session_persistence: AgentSessionPersistenceOverride::Resume,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session.as_deref(), Some("claude-session"));
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn claude_fresh_resume_uses_new_provider_session_without_changing_wardian_id() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: wardian_core::models::AgentSessionPersistence::Fresh,
            ..Default::default()
        })
        .expect("save shell settings");

        let mut config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "wardian-agent-id".to_string(),
            resume_session: Some("old-claude-session".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.session_id, "wardian-agent-id");
        assert_eq!(config.resume_session, None);
        assert_ne!(
            config.fresh_provider_session_id.as_deref(),
            Some("wardian-agent-id")
        );
        assert!(config.fresh_provider_session_id.is_some());
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn prepare_clear_config_forces_fresh_resume_and_clears_runtime_fields() {
        let mut config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "wardian-agent-id".to_string(),
            resume_session: Some("old-claude-session".to_string()),
            session_persistence: AgentSessionPersistenceOverride::Resume,
            is_off: true,
            ..Default::default()
        };

        prepare_clear_config(&mut config).expect("prepare clear config");

        assert_eq!(config.session_id, "wardian-agent-id");
        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.session_persistence,
            AgentSessionPersistenceOverride::Resume
        );
        assert!(config.fresh_provider_session_id.is_some());
        assert!(!config.is_off);
    }
}
