use crate::manager;
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AppState};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tauri::{AppHandle, Emitter, State};
use wardian_core::models::{
    AgentConfig, AgentSessionPersistence, AgentSessionPersistenceOverride, AgentTelemetry,
    DeployedSkillRef, ProviderConfig,
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

enum CloneProfileCopyPlan<'a> {
    None,
    Profile,
    Custom(&'a CloneProfileSelection),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentWorktreeSummary {
    pub id: String,
    pub name: String,
    pub source_folder: String,
    pub worktree_folder: String,
    pub member_agent_ids: Vec<String>,
    pub can_delete: bool,
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

fn detach_agent_for_kill(
    agents: &mut HashMap<String, ActiveAgent>,
    order: &mut Vec<String>,
    input_senders: &std::sync::RwLock<HashMap<String, tokio::sync::mpsc::Sender<Vec<u8>>>>,
    session_id: &str,
) -> Option<ActiveAgent> {
    let agent = agents.remove(session_id)?;
    if let Ok(mut senders) = input_senders.write() {
        senders.remove(session_id);
    }
    order.retain(|id| id != session_id);
    Some(agent)
}

async fn lock_agent_lifecycle(
    state: &AppState,
    session_id: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lifecycle_lock = {
        let mut locks = state.agent_lifecycle_locks.lock().await;
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lifecycle_lock.lock_owned().await
}

struct PreparedAgentClear {
    termination: ActiveAgent,
    config: AgentConfig,
    init_timestamp: Option<String>,
}

fn take_agent_runtime_for_termination(agent: &mut ActiveAgent) -> ActiveAgent {
    ActiveAgent {
        config: agent.config.clone(),
        child_process: agent.child_process.take(),
        background_processes: std::mem::take(&mut agent.background_processes),
        pty_master: agent.pty_master.take(),
        stdin_tx: agent.stdin_tx.take(),
        output_buffer: agent.output_buffer.clone(),
        process_id: agent.process_id.take(),
        query_count: agent.query_count.clone(),
        init_timestamp: agent.init_timestamp.clone(),
        current_status: agent.current_status.clone(),
        last_status_at: agent.last_status_at.clone(),
        watch_state: agent.watch_state.clone(),
        terminal_title: agent.terminal_title.clone(),
        last_output_at: agent.last_output_at.clone(),
        log_path: agent.log_path.clone(),
        log_last_modified: agent.log_last_modified.clone(),
        #[cfg(windows)]
        job_object: agent.job_object.take(),
    }
}

fn prepare_agent_for_clear(agent: &mut ActiveAgent) -> PreparedAgentClear {
    let termination = take_agent_runtime_for_termination(agent);
    let config = agent.config.lock().unwrap().clone();
    let init_timestamp = agent.init_timestamp.lock().unwrap().clone();

    if let Ok(mut buf) = agent.output_buffer.lock() {
        buf.clear();
    }
    if let Ok(mut title) = agent.terminal_title.lock() {
        title.clear();
    }
    agent.current_status =
        std::sync::Arc::new(std::sync::Mutex::new("Processing...".to_string()));
    if let Ok(mut count) = agent.query_count.lock() {
        *count = 0;
    }
    if let Ok(mut log_path) = agent.log_path.lock() {
        *log_path = None;
    }
    if let Ok(mut log_last_modified) = agent.log_last_modified.lock() {
        *log_last_modified = None;
    }
    if let Ok(mut watch_state) = agent.watch_state.lock() {
        watch_state.clear();
    }

    PreparedAgentClear {
        termination,
        config,
        init_timestamp,
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
    let source_provider = config.provider.clone();
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
    if config.provider != source_provider {
        config.reset_provider_config_for_provider();
        config.custom_args = None;
    } else {
        config.custom_args =
            clone_custom_args_without_provider_memory(config.custom_args.as_deref());
    }
    config.resume_session = None;
    config.fresh_provider_session_id = None;
    clear_codex_cleared_provider_sessions(&mut config);
    config.system_include_directories = None;
    config.opencode_port = None;
    if config.provider == "opencode" {
        if let wardian_core::models::ProviderConfig::OpenCode(opencode) =
            &mut config.provider_config
        {
            opencode.port = None;
        }
    }
    config.is_off = !start;
    config.mark_provider_config_nested_for_save();
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
    let lower = normalized.to_ascii_lowercase();
    matches!(
        first,
        "habitat"
            | ".codex"
            | ".claude"
            | ".gemini"
            | ".opencode"
            | "codex"
            | "claude"
            | "gemini"
            | "opencode"
            | "logs"
            | "telemetry"
            | "provider-bootstrap"
    ) || normalized == ".agents/skills"
        || normalized.starts_with(".agents/skills/")
        || lower.ends_with(".jsonl")
        || lower.ends_with(".log")
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
    let skills = crate::commands::library::list_deployed_skill_refs_for_target_strict(
        "agent",
        source_session_id,
    )?;

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

fn clone_normalize_selected_profile_file(path: &str) -> Result<String, String> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("Selected clone file path cannot be empty.".to_string());
    }
    let candidate = std::path::Path::new(&normalized);
    if candidate.is_absolute() {
        return Err(format!(
            "Selected clone file path is absolute: {normalized}"
        ));
    }
    if normalized.len() >= 3
        && normalized.as_bytes()[1] == b':'
        && normalized.as_bytes()[0].is_ascii_alphabetic()
        && normalized.as_bytes()[2] == b'/'
    {
        return Err(format!(
            "Selected clone file path is absolute: {normalized}"
        ));
    }
    for component in candidate.components() {
        if matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        ) {
            return Err(format!("Selected clone file path is invalid: {normalized}"));
        }
    }
    Ok(normalized)
}

fn clone_join_normalized_relative_path(
    root: &std::path::Path,
    normalized: &str,
) -> std::path::PathBuf {
    normalized
        .split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment))
}

fn clone_validate_selected_profile_files(
    wardian_home: &std::path::Path,
    source_session_id: &str,
    selected_files: &[String],
) -> Result<Vec<String>, String> {
    let source_root = wardian_home.join("agents").join(source_session_id);
    let canonical_root = source_root.canonicalize().map_err(|e| e.to_string())?;
    let allowlist = flatten_clone_file_paths(&clone_collect_eligible_file_tree(
        wardian_home,
        source_session_id,
    )?)
    .into_iter()
    .collect::<std::collections::HashSet<_>>();
    let mut validated = Vec::with_capacity(selected_files.len());

    for selected_file in selected_files {
        let normalized = clone_normalize_selected_profile_file(selected_file)?;
        if !allowlist.contains(&normalized) {
            return Err(format!("Selected clone file is not eligible: {normalized}"));
        }
        let source_path = clone_join_normalized_relative_path(&source_root, &normalized);
        let metadata = std::fs::symlink_metadata(&source_path).map_err(|e| e.to_string())?;
        if clone_path_is_link_or_reparse(&metadata) || std::fs::read_link(&source_path).is_ok() {
            return Err(format!("Selected clone file is a link: {normalized}"));
        }
        if !metadata.is_file() {
            return Err(format!("Selected clone path is not a file: {normalized}"));
        }
        let canonical = source_path.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(&canonical_root) {
            return Err(format!(
                "Selected clone file escapes the source agent: {normalized}"
            ));
        }
        validated.push(normalized);
    }

    Ok(validated)
}

fn clone_copy_selected_agent_profile_files(
    wardian_home: &std::path::Path,
    source_session_id: &str,
    destination_session_id: &str,
    selected_files: &[String],
) -> Result<(), String> {
    let selected_files =
        clone_validate_selected_profile_files(wardian_home, source_session_id, selected_files)?;
    let source_root = wardian_home.join("agents").join(source_session_id);
    let destination_root = wardian_home.join("agents").join(destination_session_id);
    std::fs::create_dir_all(&destination_root).map_err(|e| e.to_string())?;

    for selected_file in selected_files {
        clone_copy_file(
            &clone_join_normalized_relative_path(&source_root, &selected_file),
            &clone_join_normalized_relative_path(&destination_root, &selected_file),
        )?;
    }

    Ok(())
}

fn clone_match_selected_agent_skills(
    deployed_skills: &[DeployedSkillRef],
    selected_skills: &[DeployedSkillRef],
) -> Result<Vec<DeployedSkillRef>, String> {
    let mut matched = Vec::with_capacity(selected_skills.len());
    for selected_skill in selected_skills {
        let deployed = deployed_skills
            .iter()
            .find(|deployed| {
                deployed.name == selected_skill.name
                    && deployed.source_path == selected_skill.source_path
            })
            .ok_or_else(|| {
                format!(
                    "Selected skill '{}' is not deployed on the source agent.",
                    selected_skill.name
                )
            })?;
        matched.push(deployed.clone());
    }
    Ok(matched)
}

fn clone_validate_selected_agent_skills(
    source_session_id: &str,
    selected_skills: &[DeployedSkillRef],
) -> Result<Vec<DeployedSkillRef>, String> {
    let deployed_skills = crate::commands::library::list_deployed_skill_refs_for_target_strict(
        "agent",
        source_session_id,
    )?;
    clone_match_selected_agent_skills(&deployed_skills, selected_skills)
}

fn clone_copy_selected_agent_skills(
    wardian_home: &std::path::Path,
    source_session_id: &str,
    destination_session_id: &str,
    selected_skills: &[DeployedSkillRef],
) -> Result<(), String> {
    let selected_skills = clone_validate_selected_agent_skills(source_session_id, selected_skills)?;

    for selected_skill in selected_skills {
        if let Some(source_path) = selected_skill.source_path {
            crate::commands::library::deploy_skill_from_library(
                &source_path,
                "agent",
                destination_session_id,
            )?;
        } else {
            clone_copy_directory_recursive(
                &wardian_home
                    .join("agents")
                    .join(source_session_id)
                    .join(".agents")
                    .join("skills")
                    .join(&selected_skill.name),
                &wardian_home
                    .join("agents")
                    .join(destination_session_id)
                    .join(".agents")
                    .join("skills")
                    .join(&selected_skill.name),
            )?;
        }
    }

    Ok(())
}

fn clone_copy_profile_plan(
    wardian_home: &std::path::Path,
    source_session_id: &str,
    destination_session_id: &str,
    plan: CloneProfileCopyPlan<'_>,
) -> Result<(), String> {
    match plan {
        CloneProfileCopyPlan::None => Ok(()),
        CloneProfileCopyPlan::Profile => {
            clone_copy_agent_profile_files(wardian_home, source_session_id, destination_session_id)
        }
        CloneProfileCopyPlan::Custom(selection) => {
            clone_copy_selected_agent_profile_files(
                wardian_home,
                source_session_id,
                destination_session_id,
                &selection.files,
            )?;
            clone_copy_selected_agent_skills(
                wardian_home,
                source_session_id,
                destination_session_id,
                &selection.skills,
            )
        }
    }
}

fn clone_cleanup_created_profile_dirs(created_profile_dirs: &[std::path::PathBuf]) {
    for profile_dir in created_profile_dirs.iter().rev() {
        clone_remove_existing_path(profile_dir);
    }
}

fn clone_refresh_profile_system_include_directories(
    config: &mut AgentConfig,
    session_id: &str,
    should_copy_profile: bool,
) {
    if should_copy_profile {
        config.system_include_directories = Some(
            crate::utils::fs::resolve_system_include_directories(&config.agent_class, session_id),
        );
    }
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
        if provider_uses_manual_session_id(provider_name) && !session_id.trim().is_empty() {
            Some(session_id.to_string())
        } else {
            None
        }
    })
}

fn provider_uses_manual_session_id(provider_name: &str) -> bool {
    matches!(provider_name, "claude" | "gemini")
}

fn provider_uses_generated_session_id(provider_name: &str) -> bool {
    matches!(
        provider_name,
        "claude" | "codex" | "gemini" | "mock" | "opencode" | "antigravity"
    )
}

fn ensure_provider_available_before_session_bootstrap(provider_name: &str) -> Result<(), String> {
    crate::providers::readiness::ensure_provider_available_for_launch(provider_name)
}

fn provider_needs_obtain_session_id_on_clear(_provider_name: &str) -> bool {
    false
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
    if !provider_uses_manual_session_id(provider) {
        return;
    }

    let promoted = {
        let mut new_config = new_active.config.lock().unwrap();
        if let Some(fresh_provider_session_id) = new_config.fresh_provider_session_id.take() {
            new_config.resume_session = Some(fresh_provider_session_id);
            true
        } else {
            false
        }
    };

    if promoted {
        if let Ok(mut log_path) = new_active.log_path.lock() {
            *log_path = None;
        }
        if let Ok(mut log_last_modified) = new_active.log_last_modified.lock() {
            *log_last_modified = None;
        }
    }
}

#[cfg(test)]
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

#[cfg(test)]
fn agent_status_update_payload(session_id: &str, current_status: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "current_status": current_status,
    })
}

fn terminal_cleared_payload(session_id: &str) -> serde_json::Value {
    serde_json::json!({ "session_id": session_id })
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
        let log_path_snap = log_path_snap.or_else(|| {
            manager::opencode_log_dirs()
                .into_iter()
                .find_map(|dir| manager::opencode_log_path_in(&dir, &config.session_id))
        });
        if let Some(log_path) = log_path_snap {
            if let Some(ses_id) = manager::opencode_extract_created_session_id(&log_path) {
                config.resume_session = Some(ses_id);
            }
        }
    }
}

#[cfg(test)]
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
    strip_windows_verbatim_prefix(path.to_string_lossy().replace('\\', "/"))
}

fn normalize_existing_workspace_record_path(path: &std::path::Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    normalize_workspace_record_path(&path)
}

fn normalize_maybe_existing_workspace_record_path(path: &str) -> String {
    let normalized = strip_windows_verbatim_prefix(path.trim().replace('\\', "/"));
    let path = std::path::Path::new(&normalized);
    if path.exists() {
        return normalize_existing_workspace_record_path(path);
    }
    normalized
}

fn strip_windows_verbatim_prefix(path: String) -> String {
    if let Some(stripped) = path.strip_prefix("//?/UNC/") {
        return format!("//{stripped}");
    }
    if let Some(stripped) = path.strip_prefix("//?/") {
        return stripped.to_string();
    }
    path
}

fn resolve_agent_worktree_path(
    wardian_home: &std::path::Path,
    session_id: &str,
    worktree_name: Option<&str>,
    default_name: &str,
) -> std::path::PathBuf {
    let slug = worktree_name
        .map(slugify_worktree_name)
        .filter(|slug| !slug.is_empty())
        .or_else(|| {
            let default_slug = slugify_worktree_name(default_name);
            (!default_slug.is_empty()).then_some(default_slug)
        })
        .unwrap_or_else(|| "worktree".to_string());

    wardian_home
        .join("agents")
        .join(session_id)
        .join("worktrees")
        .join(slug)
}

fn slugify_worktree_name(name: &str) -> String {
    let mut slug = String::new();
    let mut previous_was_hyphen = false;

    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_was_hyphen = false;
        } else if (ch.is_ascii_whitespace() || ch == '-')
            && !slug.is_empty()
            && !previous_was_hyphen
        {
            slug.push('-');
            previous_was_hyphen = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        slug = "agent".to_string();
    }

    slug
}

pub(crate) fn resolve_agent_worktree_branch_name(worktree_name: &str) -> String {
    let slug = slugify_worktree_name(worktree_name);
    format!("wardian/{slug}")
}

fn enable_worktree_config(config: &mut AgentConfig, worktree_path: &std::path::Path) {
    let source_folder = config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| config.folder.clone());
    if config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        config.git_worktree_source = Some(source_folder);
    }
    let worktree_folder = normalize_workspace_record_path(worktree_path);
    config.git_worktree = Some(true);
    config.folder = worktree_folder.clone();
    config.git_worktree_folder = Some(worktree_folder);
}

fn disable_worktree_config(config: &mut AgentConfig) -> Result<(), String> {
    let source = config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .ok_or_else(|| {
            "Cannot disable worktree because the original workspace is not recorded.".to_string()
        })?
        .to_string();

    config.git_worktree = Some(false);
    config.folder = source;
    config.git_worktree_source = None;
    config.git_worktree_folder = None;
    Ok(())
}

fn collect_agent_worktrees(configs: &[AgentConfig]) -> Vec<AgentWorktreeSummary> {
    let mut summaries: BTreeMap<String, AgentWorktreeSummary> = BTreeMap::new();

    for config in configs {
        if config.git_worktree != Some(true) {
            continue;
        }
        let Some(worktree_folder) = config
            .git_worktree_folder
            .as_deref()
            .map(str::trim)
            .filter(|folder| !folder.is_empty())
        else {
            continue;
        };
        let source_folder = config
            .git_worktree_source
            .as_deref()
            .map(str::trim)
            .filter(|folder| !folder.is_empty())
            .unwrap_or(config.folder.trim());
        if source_folder.is_empty() {
            continue;
        }

        let normalized_worktree = normalize_maybe_existing_workspace_record_path(worktree_folder);
        let entry = summaries
            .entry(normalized_worktree.clone())
            .or_insert_with(|| {
                let fallback_name = std::path::Path::new(worktree_folder)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(worktree_folder)
                    .to_string();
                let name = if fallback_name == "worktree" && !config.session_name.trim().is_empty()
                {
                    format!("{} worktree", config.session_name)
                } else {
                    fallback_name
                };
                AgentWorktreeSummary {
                    id: normalized_worktree.clone(),
                    name,
                    source_folder: normalize_maybe_existing_workspace_record_path(source_folder),
                    worktree_folder: normalized_worktree.clone(),
                    member_agent_ids: Vec::new(),
                    can_delete: false,
                }
            });
        entry.member_agent_ids.push(config.session_id.clone());
    }

    summaries.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveredGitWorktree {
    source_folder: String,
    worktree_folder: String,
}

fn is_under_wardian_agent_worktree_root(wardian_home: &std::path::Path, path: &str) -> bool {
    let normalized_home =
        normalize_path_for_prefix_compare(&normalize_existing_workspace_record_path(wardian_home));
    let normalized_path =
        normalize_path_for_prefix_compare(&normalize_maybe_existing_workspace_record_path(path));
    let prefix = format!("{normalized_home}/agents/");
    if !normalized_path.starts_with(&prefix) {
        return false;
    }

    let relative = &normalized_path[prefix.len()..];
    let parts = relative.split('/').collect::<Vec<_>>();
    parts.len() >= 3 && parts[1] == "worktrees" && !parts[0].is_empty() && !parts[2].is_empty()
}

fn normalize_path_for_prefix_compare(path: &str) -> String {
    let normalized = path
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_string();

    #[cfg(windows)]
    {
        let normalized = normalized.to_ascii_lowercase();
        let bytes = normalized.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/'
        {
            let drive = bytes[1] as char;
            return format!("{drive}:{}", &normalized[2..]);
        }
        normalized
    }

    #[cfg(not(windows))]
    {
        normalized
    }
}

fn source_folder_for_config(config: &AgentConfig) -> Option<String> {
    if let Some(source) = config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        return Some(normalize_maybe_existing_workspace_record_path(source));
    }

    let folder = config.folder.trim();
    if folder.is_empty() {
        None
    } else {
        Some(normalize_maybe_existing_workspace_record_path(folder))
    }
}

fn collect_agent_worktrees_with_discovered(
    configs: &[AgentConfig],
    wardian_home: &std::path::Path,
    discovered: Vec<DiscoveredGitWorktree>,
) -> Vec<AgentWorktreeSummary> {
    let mut summaries = collect_agent_worktrees(configs)
        .into_iter()
        .map(|summary| (summary.worktree_folder.clone(), summary))
        .collect::<BTreeMap<_, _>>();

    for worktree in discovered {
        let normalized_worktree =
            normalize_maybe_existing_workspace_record_path(&worktree.worktree_folder);

        summaries
            .entry(normalized_worktree.clone())
            .or_insert_with(|| {
                let name = std::path::Path::new(&normalized_worktree)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or("worktree")
                    .to_string();
                AgentWorktreeSummary {
                    id: normalized_worktree.clone(),
                    name,
                    source_folder: normalize_maybe_existing_workspace_record_path(
                        &worktree.source_folder,
                    ),
                    worktree_folder: normalized_worktree.clone(),
                    member_agent_ids: Vec::new(),
                    can_delete: is_under_wardian_agent_worktree_root(
                        wardian_home,
                        &normalized_worktree,
                    ),
                }
            });
    }

    summaries.into_values().collect()
}

fn discover_git_worktrees_for_configs(
    configs: &[AgentConfig],
    _wardian_home: &std::path::Path,
) -> Vec<DiscoveredGitWorktree> {
    let sources = configs
        .iter()
        .filter_map(source_folder_for_config)
        .collect::<BTreeSet<_>>();

    let mut discovered = BTreeMap::<String, DiscoveredGitWorktree>::new();
    for source in sources {
        let source_path = std::path::Path::new(&source);
        let Ok(worktrees) = crate::commands::git::list_git_worktrees(source_path) else {
            continue;
        };

        for worktree in worktrees {
            let normalized = normalize_discovered_git_worktree_path(&worktree.path);
            if workspace_paths_match(&source, &normalized) {
                continue;
            }
            discovered
                .entry(normalized.clone())
                .or_insert_with(|| DiscoveredGitWorktree {
                    source_folder: source.clone(),
                    worktree_folder: normalized,
                });
        }
    }

    discovered.into_values().collect()
}

fn normalize_discovered_git_worktree_path(path: &std::path::Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    normalize_workspace_record_path(&path)
}

fn ensure_existing_worktree_is_git_registered(
    workspace_path: &std::path::Path,
    worktree_path: &std::path::Path,
) -> Result<(), String> {
    if worktree_path.exists()
        && !crate::commands::git::git_worktree_contains_path(workspace_path, worktree_path)?
    {
        return Err(
            "Worktree folder already exists but is not registered with Git for this workspace."
                .to_string(),
        );
    }
    Ok(())
}

fn workspace_paths_match(left: &str, right: &str) -> bool {
    normalize_path_for_prefix_compare(&normalize_maybe_existing_workspace_record_path(left))
        == normalize_path_for_prefix_compare(&normalize_maybe_existing_workspace_record_path(right))
}

fn find_assignable_worktree(
    configs: &[AgentConfig],
    wardian_home: &std::path::Path,
    worktree_folder: &str,
    discovered: Vec<DiscoveredGitWorktree>,
) -> Option<AgentWorktreeSummary> {
    let normalized_worktree_folder =
        normalize_maybe_existing_workspace_record_path(worktree_folder);
    collect_agent_worktrees_with_discovered(configs, wardian_home, discovered)
        .into_iter()
        .find(|worktree| worktree.worktree_folder == normalized_worktree_folder)
}

fn validate_assignable_worktree_for_agent(
    source_folder: &str,
    managed_worktree: &AgentWorktreeSummary,
    worktree_path: &std::path::Path,
) -> Result<(), String> {
    if !workspace_paths_match(source_folder, &managed_worktree.source_folder) {
        return Err(
            "Cannot assign an agent to a worktree from another source workspace".to_string(),
        );
    }

    ensure_existing_worktree_is_git_registered(
        std::path::Path::new(&managed_worktree.source_folder),
        worktree_path,
    )
}

fn validate_deletable_agent_worktree(
    wardian_home: &std::path::Path,
    managed_worktree: &AgentWorktreeSummary,
) -> Result<(), String> {
    if !managed_worktree.member_agent_ids.is_empty() {
        return Err("Cannot delete a worktree while agents are assigned to it.".to_string());
    }

    if !is_under_wardian_agent_worktree_root(wardian_home, &managed_worktree.worktree_folder) {
        return Err("Only Wardian agent worktrees can be deleted from Wardian.".to_string());
    }

    if workspace_paths_match(
        &managed_worktree.source_folder,
        &managed_worktree.worktree_folder,
    ) {
        return Err("Refusing to delete the source checkout as a worktree.".to_string());
    }

    ensure_existing_worktree_is_git_registered(
        std::path::Path::new(&managed_worktree.source_folder),
        std::path::Path::new(&managed_worktree.worktree_folder),
    )
}

fn assign_worktree_config(config: &mut AgentConfig, worktree_folder: &str) -> Result<(), String> {
    let worktree_folder = worktree_folder.trim();
    if worktree_folder.is_empty() {
        return Err("Worktree folder is required".to_string());
    }
    let source_folder = config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| config.folder.clone());
    if config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        config.git_worktree_source = Some(source_folder);
    }
    let worktree_folder = worktree_folder.replace('\\', "/");
    config.git_worktree = Some(true);
    config.folder = worktree_folder.clone();
    config.git_worktree_folder = Some(worktree_folder);
    Ok(())
}

fn normalize_spawn_folder(folder: &str) -> Result<String, String> {
    if folder.trim().is_empty() {
        return Ok(String::new());
    }

    crate::utils::fs::validate_workspace_path(std::path::Path::new(folder))
        .map(|path| normalize_workspace_record_path(&path))
}

fn normalize_clone_folder_override(folder: Option<String>) -> Result<Option<String>, String> {
    folder.as_deref().map(normalize_spawn_folder).transpose()
}

fn codex_cleared_provider_sessions(config: &AgentConfig) -> Vec<String> {
    config.codex_config().cleared_provider_sessions
}

fn clear_codex_cleared_provider_sessions(config: &mut AgentConfig) {
    if config.provider == "codex" || matches!(config.provider_config, ProviderConfig::Codex(_)) {
        config
            .codex_config_mut_preserve_encoding()
            .cleared_provider_sessions
            .clear();
    }
    config.codex_cleared_provider_sessions.clear();
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
        if provider_uses_manual_session_id(&config.provider) {
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
            let cleared_provider_sessions = codex_cleared_provider_sessions(config);
            rs.trim().is_empty()
                || rs == config.session_id
                || !codex_provider_session_is_new(rs, &cleared_provider_sessions)
                || !manager::codex_session_exists_in_agent_home(&config.session_id, rs)
        })
    {
        config.resume_session = None;
    }

    if config.resume_session.is_none() && config.provider == "codex" {
        if let Some((provider_session_id, _updated_at)) = manager::latest_codex_session_index_entry(
            &config.session_id,
        )?
        .filter(|(provider_session_id, _updated_at)| {
            let cleared_provider_sessions = codex_cleared_provider_sessions(config);
            codex_provider_session_is_new(provider_session_id, &cleared_provider_sessions)
                && manager::codex_session_exists_in_agent_home(
                    &config.session_id,
                    provider_session_id,
                )
        }) {
            config.resume_session = Some(provider_session_id);
            clear_codex_cleared_provider_sessions(config);
        }
    }

    if config.resume_session.is_none() {
        let should_fallback = match config.provider.as_str() {
            "opencode" => config.session_id.starts_with("ses_"),
            "codex" | "antigravity" => false,
            _ => true,
        };
        if should_fallback {
            config.resume_session = Some(config.session_id.clone());
        }
    }

    Ok(())
}

pub(crate) fn prepare_restored_config_for_spawn(config: &mut AgentConfig) -> Result<(), String> {
    if config.is_off {
        return Ok(());
    }

    prepare_resume_config(config)
}

fn prepare_resume_config_for_runtime(
    config: &mut AgentConfig,
    query_count: usize,
) -> Result<(), String> {
    if config.provider == "gemini" && query_count == 0 && !config.is_off {
        config.is_off = false;
        config.resume_session = None;
        config.fresh_provider_session_id = Some(uuid::Uuid::new_v4().to_string());
        return Ok(());
    }

    prepare_resume_config(config)
}

fn prepare_clear_config(config: &mut AgentConfig) -> Result<(), String> {
    config.is_off = false;
    config.resume_session = None;
    config.fresh_provider_session_id = None;
    clear_codex_cleared_provider_sessions(config);
    if provider_uses_manual_session_id(&config.provider) {
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
                            &codex_cleared_provider_sessions(&config),
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
                            &codex_cleared_provider_sessions(&config),
                        )
                    })
            };

            if let Some(provider_session_id) = provider_session_id {
                actual_resume = Some(provider_session_id.clone());
                config.resume_session = Some(provider_session_id.clone());
                clear_codex_cleared_provider_sessions(&mut config);
                {
                    let mut cfg = active_agent.config.lock().unwrap();
                    cfg.resume_session = Some(provider_session_id.clone());
                    clear_codex_cleared_provider_sessions(&mut cfg);
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
        if config.provider == "opencode" {
            let opencode = cfg.opencode_config();
            config.opencode_port = opencode.port;
            if let wardian_core::models::ProviderConfig::OpenCode(target) =
                &mut config.provider_config
            {
                target.port = opencode.port;
            }
        }
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
pub fn list_provider_readiness(
) -> Result<Vec<crate::providers::readiness::ProviderReadiness>, String> {
    Ok(crate::providers::readiness::list_provider_readiness())
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

    let provider_name = config_override
        .as_ref()
        .map(|c| c.provider.clone())
        .unwrap_or_else(|| "claude".to_string());
    ensure_provider_available_before_session_bootstrap(&provider_name)?;

    let name_reservation =
        reserve_spawn_session_name(&state, &requested_session_name, &agent_class).await?;
    let session_name = name_reservation.session_name.clone();

    manager::log_debug(&format!(
        "[WARDIAN] spawn_agent called for session name: {}, class: {}",
        session_name, agent_class
    ));
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
    config.normalize_provider_config_for_provider();
    config.mark_provider_config_nested_for_save();
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

    let folder_override = normalize_clone_folder_override(req.folder)?;
    let profile_selection = req.profile_selection;
    let profile_copy_plan = match profile_selection.as_ref() {
        Some(selection) => CloneProfileCopyPlan::Custom(selection),
        None if req.mode == CloneAgentMode::Profile => CloneProfileCopyPlan::Profile,
        None => CloneProfileCopyPlan::None,
    };
    let should_copy_profile = !matches!(profile_copy_plan, CloneProfileCopyPlan::None);

    let mut config = clone_sanitize_config(
        &source_config,
        session_name,
        req.provider,
        folder_override,
        req.agent_class,
        req.start.unwrap_or(true),
    );
    let provider_name = config.provider.clone();
    ensure_provider_available_before_session_bootstrap(&provider_name)?;
    let mut actual_resume = None;
    let profile_home = if should_copy_profile {
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
    let mut created_profile_dirs = Vec::new();

    let session_id = if provider_uses_generated_session_id(&provider_name) {
        let generated_session_id = uuid::Uuid::new_v4().to_string();
        config.session_id = generated_session_id.clone();
        if let Some(home) = profile_home.as_ref() {
            clone_ensure_profile_destination_available(home, &generated_session_id, None)?;
            created_profile_dirs.push(home.join("agents").join(&generated_session_id));
            if let Err(error) = clone_copy_profile_plan(
                home,
                &source_session_id,
                &generated_session_id,
                match profile_selection.as_ref() {
                    Some(selection) => CloneProfileCopyPlan::Custom(selection),
                    None => CloneProfileCopyPlan::Profile,
                },
            ) {
                clone_cleanup_created_profile_dirs(&created_profile_dirs);
                return Err(error);
            }
        }
        generated_session_id
    } else {
        if let (Some(home), Some(provisional_session_id)) = (
            profile_home.as_ref(),
            provisional_profile_session_id.as_ref(),
        ) {
            config.session_id = provisional_session_id.clone();
            clone_ensure_profile_destination_available(home, provisional_session_id, None)?;
            created_profile_dirs.push(home.join("agents").join(provisional_session_id));
            if let Err(error) = clone_copy_profile_plan(
                home,
                &source_session_id,
                provisional_session_id,
                match profile_selection.as_ref() {
                    Some(selection) => CloneProfileCopyPlan::Custom(selection),
                    None => CloneProfileCopyPlan::Profile,
                },
            ) {
                clone_cleanup_created_profile_dirs(&created_profile_dirs);
                return Err(error);
            }
            config.system_include_directories =
                Some(crate::utils::fs::resolve_system_include_directories(
                    &config.agent_class,
                    provisional_session_id,
                ));
        }
        let cwd = crate::utils::fs::resolve_cwd(&config.folder, "");
        let real_sid = manager::obtain_session_id(&cwd, Some(&config.agent_class), Some(&config))
            .await
            .map_err(|e| {
                clone_cleanup_created_profile_dirs(&created_profile_dirs);
                format!("Failed to initialize the provider session: {}", e)
            })?;
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
        clone_cleanup_created_profile_dirs(&created_profile_dirs);
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
        )
        .inspect_err(|_| clone_cleanup_created_profile_dirs(&created_profile_dirs))?;
        let final_profile_dir = home.join("agents").join(&session_id);
        if !created_profile_dirs
            .iter()
            .any(|existing| existing == &final_profile_dir)
        {
            created_profile_dirs.push(final_profile_dir);
        }
        if let Err(error) = clone_copy_profile_plan(
            home,
            &source_session_id,
            &session_id,
            match profile_selection.as_ref() {
                Some(selection) => CloneProfileCopyPlan::Custom(selection),
                None => CloneProfileCopyPlan::Profile,
            },
        ) {
            clone_cleanup_created_profile_dirs(&created_profile_dirs);
            return Err(error);
        }
        if let Some(provisional_session_id) = provisional_profile_session_id
            .as_deref()
            .filter(|id| *id != session_id)
        {
            let provisional_root = home.join("agents").join(provisional_session_id);
            clone_remove_existing_path(&provisional_root);
            created_profile_dirs.retain(|dir| dir != &provisional_root);
        }
        clone_refresh_profile_system_include_directories(&mut config, &session_id, true);
    }

    let registered = register_new_agent(
        config,
        actual_resume,
        &state,
        &app,
        generated_session_name.then_some(source_config.session_name.as_str()),
        None,
        AgentOrderPlacement::After(&source_session_id),
    )
    .await;
    match registered {
        Ok(config) => {
            if let Err(error) = crate::commands::watchlist::preserve_clone_team_placement(
                &app,
                &source_session_id,
                &config.session_id,
            ) {
                manager::log_debug(&format!(
                    "[WARDIAN] Failed to preserve clone team placement for {} cloned from {}: {}",
                    config.session_id, source_session_id, error
                ));
            }
            Ok(config)
        }
        Err(error) => {
            clone_cleanup_created_profile_dirs(&created_profile_dirs);
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn get_agent_clone_preview(
    source_session_id: String,
    state: State<'_, AppState>,
) -> Result<AgentClonePreview, String> {
    let source_session_id = source_session_id.trim().to_string();
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
    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find Wardian home".to_string())?;

    build_agent_clone_preview(
        &wardian_home,
        &source_session_id,
        &source_config,
        &existing_names,
    )
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
    let _lifecycle_guard = lock_agent_lifecycle(&state, &session_id).await;
    let (agent, state_snapshot) = {
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        let agent =
            detach_agent_for_kill(&mut agents, &mut order, &state.input_senders, &session_id);
        let state_snapshot = agent
            .is_some()
            .then(|| manager::state_configs_snapshot(&agents, &order));
        (agent, state_snapshot)
    };
    if let Some(snapshot) = state_snapshot {
        manager::save_state_snapshot(&app, &snapshot);
    }
    if agent.is_some() {
        state.remove_agent_delivery_state(&session_id).await;
    }

    #[allow(unused_mut)]
    if let Some(mut agent) = agent {
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
    let _lifecycle_guard = lock_agent_lifecycle(&state, &session_id).await;
    let (mut termination, state_snapshot, status_arc) = {
        let mut agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;

        let Some(agent) = agents.get_mut(&session_id) else {
            return Err(format!("Agent {} not found", session_id));
        };

        let termination = take_agent_runtime_for_termination(agent);
        let status_arc = agent.current_status.clone();
        {
            let mut config = agent.config.lock().unwrap();
            config.is_off = true;
        }

        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        let state_snapshot = manager::state_configs_snapshot(&agents, &order);
        (termination, state_snapshot, status_arc)
    };
    manager::save_state_snapshot(&app, &state_snapshot);
    manager::set_agent_status(&app, &session_id, &status_arc, "Off");

    manager::terminate_active_agent_process(&mut termination);

    // For opencode: capture the real ses_xxx session ID from the log so
    // resume can pass --session ses_xxx rather than the internal UUID.
    capture_opencode_pause_resume_session(&termination);

    let state_snapshot = {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        manager::state_configs_snapshot(&agents, &order)
    };
    manager::save_state_snapshot(&app, &state_snapshot);

    let _ = app.emit("agents-updated", ());
    Ok(())
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
    let _lifecycle_guard = lock_agent_lifecycle(&state, &session_id).await;
    let snapshot = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        capture_resume_runtime_snapshot(agent)
    };

    let mut config = snapshot.config;
    prepare_resume_config_for_runtime(&mut config, snapshot.query_count)?;
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
    let mut pending_new_active = Some(new_active);
    let (mut old_agent, state_snapshot) = {
        let mut agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        if !agents.contains_key(&session_id) {
            drop(order);
            drop(agents);
            if let Some(mut active) = pending_new_active {
                manager::terminate_active_agent_process(&mut active);
            }
            return Err(format!("Agent {} not found", session_id));
        };
        let inserted = pending_new_active
            .take()
            .expect("new active agent should still be pending");
        let old_agent = agents
            .insert(session_id.clone(), inserted)
            .expect("agent should exist after contains check");
        if let Ok(mut senders) = state.input_senders.write() {
            match stdin_tx {
                Some(tx) => {
                    senders.insert(session_id.clone(), tx);
                }
                None => {
                    senders.remove(&session_id);
                }
            }
        }
        (old_agent, manager::state_configs_snapshot(&agents, &order))
    };
    manager::save_state_snapshot(&app, &state_snapshot);

    manager::terminate_active_agent_process(&mut old_agent);

    let _ = app.emit(
        "agent-terminal-cleared",
        terminal_cleared_payload(&session_id),
    );
    let _ = app.emit("agents-updated", ());
    let _ = app.emit(
        "agent-pty-output-ready",
        terminal_cleared_payload(&session_id),
    );
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
    let _lifecycle_guard = lock_agent_lifecycle(&state, &session_id).await;
    let mut prepared = {
        let mut agents = state.agents.lock().await;
        let Some(agent) = agents.get_mut(&session_id) else {
            return Err(format!("Agent {} not found", session_id));
        };

        let prepared = prepare_agent_for_clear(agent);
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        prepared
    };

    // 1. Terminate the old agent's process tree outside the global agent lock.
    manager::terminate_active_agent_process(&mut prepared.termination);

    // 2. Prepare fresh config (new provider session ID for Claude, clear resume IDs)
    let mut config = prepared.config.clone();
    let previous_codex_provider_sessions = if config.provider == "codex" {
        let mut sessions = codex_cleared_provider_sessions(&config);
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
        config
            .codex_config_mut_preserve_encoding()
            .cleared_provider_sessions = previous_codex_provider_sessions.clone();
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

    let _ = app.emit(
        "agent-terminal-cleared",
        terminal_cleared_payload(&session_id),
    );

    // 5. Spawn a FRESH process (is_restored = false) outside the global agent lock.
    // This ensures Claude uses --session-id and others start clean.
    let new_active =
        manager::spawn_agent(app.clone(), config, false, prepared.init_timestamp.clone()).await?;

    {
        let mut new_config = new_active.config.lock().unwrap();
        if provider_uses_manual_session_id(&prepared.config.provider) {
            if let Some(fresh_provider_session_id) = new_config.fresh_provider_session_id.take() {
                new_config.resume_session = Some(fresh_provider_session_id);
            }
        }
    }

    let new_stdin_tx = new_active.stdin_tx.clone();
    let db_snapshot = {
        let config = new_active.config.lock().unwrap();
        let born = new_active.init_timestamp.lock().unwrap();
        let workspace = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id)
            .to_string_lossy()
            .to_string();
        let project = wardian_core::db::project_name_from_workspace(&workspace);
        (
            config.session_id.clone(),
            config.session_name.clone(),
            config.agent_class.clone(),
            config.provider.clone(),
            workspace,
            project,
            config.is_off,
            born.clone(),
        )
    };

    let mut pending_new_active = Some(new_active);
    let (state_snapshot, mut displaced_agent) = {
        let mut agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        let Some(agent) = agents.get_mut(&session_id) else {
            drop(order);
            drop(agents);
            if let Some(mut active) = pending_new_active {
                manager::terminate_active_agent_process(&mut active);
            }
            return Err(format!("Agent {} not found", session_id));
        };
        let inserted = pending_new_active
            .take()
            .expect("new active agent should still be pending");
        let displaced_agent = std::mem::replace(agent, inserted);

        if let Ok(mut senders) = state.input_senders.write() {
            match new_stdin_tx {
                Some(tx) => {
                    senders.insert(session_id.clone(), tx);
                }
                None => {
                    senders.remove(&session_id);
                }
            }
        };
        (
            manager::state_configs_snapshot(&agents, &order),
            displaced_agent,
        )
    };
    manager::terminate_active_agent_process(&mut displaced_agent);
    manager::save_state_snapshot(&app, &state_snapshot);

    // 7. Update agent metadata in SQLite after the replacement is committed.
    let _ = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &db_snapshot.0,
        session_name: &db_snapshot.1,
        agent_class: &db_snapshot.2,
        provider: &db_snapshot.3,
        workspace: Some(&db_snapshot.4),
        project: db_snapshot.5.as_deref(),
        is_off: db_snapshot.6,
        created_at: db_snapshot.7.as_deref(),
    });

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
    new_config.validate_provider_config_matches_provider()?;
    new_config.mark_provider_config_nested_for_save();
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
pub async fn build_agent_cli_command(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let settings = crate::utils::load_shell_settings().unwrap_or_default();
    let available_shells = crate::utils::list_available_shells();
    build_agent_cli_command_for_session_id_with_shells(
        &session_id,
        state.inner(),
        &settings,
        &available_shells,
    )
    .await
}

async fn build_agent_cli_command_for_session_id_with_shells(
    session_id: &str,
    state: &AppState,
    shell_settings: &crate::utils::ShellSettings,
    available_shells: &[crate::utils::ShellOption],
) -> Result<String, String> {
    if session_id.trim().is_empty() {
        return Err("Agent session id is required".to_string());
    }

    let config = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        let config = agent.config.lock().unwrap().clone();
        config
    };

    build_agent_cli_command_with_shells(&config, shell_settings, available_shells)
}

fn build_agent_cli_command_with_shells(
    config: &AgentConfig,
    shell_settings: &crate::utils::ShellSettings,
    available_shells: &[crate::utils::ShellOption],
) -> Result<String, String> {
    if config.session_id.trim().is_empty() {
        return Err("Agent session id is required".to_string());
    }
    if config.folder.trim().is_empty() {
        return Err("Agent workspace is not configured".to_string());
    }
    validate_external_terminal_custom_args(config.custom_args.as_deref())?;

    let mut resume_config = config.clone();
    resume_config.resume_session = Some(resolve_external_resume_session(config)?);

    let provider = ProviderFactory::resolve(&resume_config.provider)?;
    let workspace_cwd =
        crate::utils::fs::resolve_cwd(&resume_config.folder, &resume_config.session_id);
    let habitat_root = crate::utils::fs::prepare_provider_habitat(
        &resume_config.provider,
        &workspace_cwd,
        &resume_config.agent_class,
        Some(&resume_config.session_id),
    )?;
    let provider_cwd = manager::interactive_provider_cwd(
        &resume_config.provider,
        &workspace_cwd,
        habitat_root.as_deref(),
        None,
    );

    let (bin, mut provider_args) = provider.get_executable();
    let spawn_args = external_terminal_spawn_args(
        &resume_config.provider,
        provider.get_spawn_args(&resume_config, true),
    );
    provider_args.extend(spawn_args);
    provider_args = manager::interactive_provider_args(
        &resume_config.provider,
        &provider_cwd,
        &workspace_cwd,
        provider_args,
    );
    let launch =
        manager::interactive_provider_launch(&resume_config.provider, &bin, &provider_args)?;
    let envs = external_terminal_env(&resume_config, habitat_root.as_deref(), &provider_cwd)?;

    crate::utils::build_copyable_program_command_with_settings(
        &launch.executable,
        &launch.args,
        &provider_cwd,
        &envs,
        shell_settings,
        available_shells,
    )
}

fn resolve_external_resume_session(config: &AgentConfig) -> Result<String, String> {
    if let Some(resume_session) = config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(resume_session.to_string());
    }

    let session_id = config.session_id.trim();
    if matches!(config.provider.as_str(), "claude" | "opencode") && !session_id.is_empty() {
        return Ok(session_id.to_string());
    }

    Err("Provider resume session is not available for this agent".to_string())
}

fn validate_external_terminal_custom_args(custom_args: Option<&str>) -> Result<(), String> {
    let Some(custom_args) = custom_args.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };

    shlex::split(custom_args)
        .map(|_| ())
        .ok_or_else(|| "Provider custom arguments are not valid shell syntax".to_string())
}

fn external_terminal_spawn_args(provider: &str, args: Vec<String>) -> Vec<String> {
    match provider {
        "claude" => strip_claude_embedded_stream_flags(args),
        "codex" => strip_codex_embedded_runtime_flags(args),
        _ => args,
    }
}

fn strip_codex_embedded_runtime_flags(args: Vec<String>) -> Vec<String> {
    let mut stripped = Vec::with_capacity(args.len());
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--no-alt-screen" {
            continue;
        }
        if arg == "--disable"
            && iter
                .peek()
                .is_some_and(|value| matches!(value.as_str(), "plugins" | "apps"))
        {
            let _ = iter.next();
            continue;
        }
        stripped.push(arg);
    }
    stripped
}

fn strip_claude_embedded_stream_flags(args: Vec<String>) -> Vec<String> {
    let mut stripped = Vec::with_capacity(args.len());
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--verbose" {
            continue;
        }
        if matches!(arg.as_str(), "--input-format" | "--output-format")
            && iter.peek().is_some_and(|value| value == "stream-json")
        {
            let _ = iter.next();
            continue;
        }
        stripped.push(arg);
    }
    stripped
}

fn external_terminal_env(
    config: &AgentConfig,
    habitat_root: Option<&std::path::Path>,
    provider_cwd: &std::path::Path,
) -> Result<Vec<(String, String)>, String> {
    let mut envs = vec![("WARDIAN_SESSION_ID".to_string(), config.session_id.clone())];
    match config.provider.as_str() {
        "claude" => envs.push((
            "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD".to_string(),
            "1".to_string(),
        )),
        "codex" => {
            if let Some(root) = habitat_root {
                envs.push((
                    "CODEX_HOME".to_string(),
                    crate::utils::fs::habitat_codex_home(root)
                        .to_string_lossy()
                        .to_string(),
                ));
            }
        }
        "opencode" => {
            envs.extend(crate::manager::opencode::opencode_interactive_env(
                provider_cwd,
                config,
            )?);
        }
        _ => {}
    }
    Ok(envs)
}

#[tauri::command]
pub async fn enable_agent_worktree(
    session_id: String,
    worktree_name: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("Session id is required".to_string());
    }

    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Unable to resolve Wardian home".to_string())?;
    let worktree_name = worktree_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let (workspace_folder, branch_name, worktree_path) = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        let config = agent.config.lock().unwrap();
        let workspace_folder = config
            .git_worktree_source
            .as_deref()
            .map(str::trim)
            .filter(|source| !source.is_empty())
            .unwrap_or(config.folder.trim())
            .to_string();
        if workspace_folder.is_empty() {
            return Err("Agent workspace is not configured".to_string());
        }
        let branch_source = worktree_name.as_deref().unwrap_or(&config.session_name);
        (
            workspace_folder,
            resolve_agent_worktree_branch_name(branch_source),
            resolve_agent_worktree_path(
                &wardian_home,
                &session_id,
                worktree_name.as_deref(),
                &config.session_name,
            ),
        )
    };

    let workspace_path = std::path::PathBuf::from(&workspace_folder);
    if worktree_path.exists() {
        ensure_existing_worktree_is_git_registered(&workspace_path, &worktree_path)?;
        crate::commands::git::setup_worktree_build_caches(&worktree_path, &workspace_path)?;
    } else {
        crate::commands::git::create_worktree_with_build_caches(
            &workspace_path,
            &worktree_path,
            &branch_name,
        )?;
        if !crate::commands::git::git_worktree_contains_path(&workspace_path, &worktree_path)? {
            return Err("Git did not register the created worktree.".to_string());
        }
    }

    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    if let Some(agent) = agents.get_mut(&session_id) {
        {
            let mut config = agent.config.lock().unwrap();
            enable_worktree_config(&mut config, &worktree_path);
        }
        manager::save_state(&app, &agents, &order);
        let _ = app.emit("agents-updated", ());
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

#[tauri::command]
pub async fn list_agent_worktrees(
    state: State<'_, AppState>,
) -> Result<Vec<AgentWorktreeSummary>, String> {
    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Unable to resolve Wardian home".to_string())?;
    let configs = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .map(|agent| agent.config.lock().unwrap().clone())
            .collect::<Vec<_>>()
    };
    let discovered = discover_git_worktrees_for_configs(&configs, &wardian_home);
    Ok(collect_agent_worktrees_with_discovered(
        &configs,
        &wardian_home,
        discovered,
    ))
}

#[tauri::command]
pub async fn assign_agent_worktree(
    session_id: String,
    worktree_folder: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("Session id is required".to_string());
    }
    let worktree_path = std::path::Path::new(worktree_folder.trim());
    if !worktree_path.is_dir() {
        return Err("Worktree folder does not exist".to_string());
    }
    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Unable to resolve Wardian home".to_string())?;

    let configs = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .map(|agent| agent.config.lock().unwrap().clone())
            .collect::<Vec<_>>()
    };
    let discovered = discover_git_worktrees_for_configs(&configs, &wardian_home);
    let managed_worktree =
        find_assignable_worktree(&configs, &wardian_home, &worktree_folder, discovered)
            .ok_or_else(|| "Worktree is not managed by Wardian".to_string())?;
    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    if let Some(agent) = agents.get_mut(&session_id) {
        {
            let mut config = agent.config.lock().unwrap();
            let source_folder = config
                .git_worktree_source
                .as_deref()
                .map(str::trim)
                .filter(|folder| !folder.is_empty())
                .unwrap_or(config.folder.trim())
                .to_string();
            validate_assignable_worktree_for_agent(
                &source_folder,
                &managed_worktree,
                worktree_path,
            )?;
            assign_worktree_config(&mut config, &worktree_folder)?;
        }
        manager::save_state(&app, &agents, &order);
        let _ = app.emit("agents-updated", ());
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

#[tauri::command]
pub async fn delete_agent_worktree(
    worktree_folder: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let worktree_folder = worktree_folder.trim().to_string();
    if worktree_folder.is_empty() {
        return Err("Worktree folder is required".to_string());
    }
    let worktree_path = std::path::Path::new(&worktree_folder);
    if !worktree_path.is_dir() {
        return Err("Worktree folder does not exist".to_string());
    }

    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Unable to resolve Wardian home".to_string())?;
    let configs = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .map(|agent| agent.config.lock().unwrap().clone())
            .collect::<Vec<_>>()
    };
    let discovered = discover_git_worktrees_for_configs(&configs, &wardian_home);
    let managed_worktree =
        find_assignable_worktree(&configs, &wardian_home, &worktree_folder, discovered)
            .ok_or_else(|| "Worktree is not managed by Wardian".to_string())?;

    validate_deletable_agent_worktree(&wardian_home, &managed_worktree)?;
    crate::commands::git::remove_worktree_without_force(
        std::path::Path::new(&managed_worktree.source_folder),
        std::path::Path::new(&managed_worktree.worktree_folder),
    )?;

    let _ = app.emit("agents-updated", ());
    Ok(())
}

#[tauri::command]
pub async fn disable_agent_worktree(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("Session id is required".to_string());
    }

    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    if let Some(agent) = agents.get_mut(&session_id) {
        {
            let mut config = agent.config.lock().unwrap();
            disable_worktree_config(&mut config)?;
        }
        manager::save_state(&app, &agents, &order);
        let _ = app.emit("agents-updated", ());
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
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
        agent_status_update_payload, assign_worktree_config,
        build_agent_cli_command_for_session_id_with_shells, build_agent_cli_command_with_shells,
        build_agent_clone_preview, capture_opencode_pause_resume_session,
        capture_resume_runtime_snapshot, clone_cleanup_created_profile_dirs,
        clone_collect_eligible_file_tree, clone_copy_agent_profile_files, clone_copy_profile_plan,
        clone_copy_selected_agent_profile_files, clone_copy_selected_agent_skills,
        clone_ensure_profile_destination_available, clone_match_selected_agent_skills,
        clone_refresh_profile_system_include_directories, clone_remove_existing_path,
        clone_sanitize_config, clone_unique_name, clone_validate_selected_agent_skills,
        clone_validate_selected_profile_files, codex_provider_session_is_new,
        collect_agent_worktrees, collect_agent_worktrees_with_discovered, detach_agent_for_kill,
        disable_worktree_config, discover_git_worktrees_for_configs, enable_worktree_config,
        ensure_existing_worktree_is_git_registered,
        ensure_provider_available_before_session_bootstrap, find_assignable_worktree,
        flatten_clone_file_paths, generated_agent_name, insert_new_agent_order,
        is_under_wardian_agent_worktree_root, lock_agent_lifecycle, mark_agent_paused_off,
        normalize_clone_folder_override, normalize_discovered_git_worktree_path,
        normalize_existing_workspace_record_path, normalize_spawn_folder,
        normalize_workspace_record_path, persisted_resume_session_for_provider,
        prepare_agent_for_clear, prepare_clear_config, prepare_restored_config_for_spawn,
        prepare_resume_config, prepare_resume_config_for_runtime,
        promote_fresh_provider_session_after_resume, provider_needs_obtain_session_id_on_clear,
        provider_uses_generated_session_id, reserve_spawn_session_name,
        resolve_agent_worktree_branch_name, resolve_agent_worktree_path,
        resolve_requested_spawn_session_name, restore_runtime_state_snapshot_after_resume,
        sync_resumed_input_sender, take_agent_runtime_for_termination, terminal_cleared_payload,
        validate_assignable_worktree_for_agent, validate_deletable_agent_worktree,
        workspace_paths_match, AgentOrderPlacement, AgentWorktreeSummary, CloneProfileCopyPlan,
        CloneProfileSelection, DiscoveredGitWorktree,
    };
    use crate::providers::GeminiProvider;
    use crate::state::{ActiveAgent, AppState};
    use crate::utils::fs::create_directory_link;
    use crate::utils::{ShellOption, ShellSettings};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use wardian_core::models::provider::AgentProvider;
    use wardian_core::models::{
        AgentConfig, AgentSessionPersistenceOverride, ClaudeProviderConfig, CodexProviderConfig,
        DeployedSkillRef, GeminiProviderConfig, OpenCodeProviderConfig, ProviderConfig,
    };

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
    fn spawn_bootstrap_readiness_error_precedes_provider_session_initialization() {
        let _lock = crate::utils::wardian_test_env_lock();
        let previous_path = std::env::var_os("PATH");
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("PATH", temp.path()) };

        let err = ensure_provider_available_before_session_bootstrap("codex")
            .expect_err("missing Codex should fail before bootstrap");

        assert!(err.contains("Codex"));
        assert!(err.contains("codex"));
        assert!(err.contains("docs/guide/provider-readiness.md"));

        match previous_path {
            Some(path) => unsafe { std::env::set_var("PATH", path) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }

    #[test]
    fn clone_bootstrap_readiness_error_precedes_provider_session_initialization() {
        let _lock = crate::utils::wardian_test_env_lock();
        let previous_path = std::env::var_os("PATH");
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("PATH", temp.path()) };

        let err = ensure_provider_available_before_session_bootstrap("codex")
            .expect_err("missing Codex should fail before clone bootstrap");

        assert!(err.contains("Codex"));
        assert!(err.contains("codex"));
        assert!(err.contains("docs/guide/provider-readiness.md"));

        match previous_path {
            Some(path) => unsafe { std::env::set_var("PATH", path) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }

    fn test_pwsh_shell() -> (ShellSettings, Vec<ShellOption>) {
        (
            ShellSettings {
                shell_id: "pwsh".to_string(),
                ..Default::default()
            },
            vec![ShellOption {
                id: "pwsh".to_string(),
                label: "PowerShell 7".to_string(),
                executable: "pwsh".to_string(),
                default_args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            }],
        )
    }

    #[test]
    fn full_agent_command_requires_configured_agent_identity() {
        let (settings, shells) = test_pwsh_shell();
        let err = build_agent_cli_command_with_shells(
            &AgentConfig {
                provider: "codex".to_string(),
                folder: "C:/repo".to_string(),
                ..Default::default()
            },
            &settings,
            &shells,
        )
        .unwrap_err();

        assert_eq!(err, "Agent session id is required");
    }

    #[test]
    fn full_agent_command_builds_codex_resume_with_cwd_and_env() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let (settings, shells) = test_pwsh_shell();

        let command = build_agent_cli_command_with_shells(
            &AgentConfig {
                session_id: "agent-1".to_string(),
                session_name: "CoderOne".to_string(),
                agent_class: "Coder".to_string(),
                folder: workspace.to_string_lossy().to_string(),
                provider: "codex".to_string(),
                resume_session: Some("provider-session".to_string()),
                ..Default::default()
            },
            &settings,
            &shells,
        )
        .expect("full command");

        assert!(command.contains("$env:WARDIAN_SESSION_ID = 'agent-1'"));
        assert!(command.contains("$env:CODEX_HOME = "));
        assert!(command.contains("Set-Location -LiteralPath "));
        assert!(command.contains("-ErrorAction Stop"));
        assert!(command.contains("resume"));
        assert!(command.contains("provider-session"));
        assert!(!command.contains("--no-alt-screen"));
        assert!(!command.contains("--disable"));
        assert!(!command.contains("plugins"));
        assert!(!command.contains("apps"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn full_agent_command_loads_saved_config_from_state() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let (settings, shells) = test_pwsh_shell();
        let state = AppState::new();
        let agent = make_test_agent();
        {
            let mut config = agent.config.lock().unwrap();
            *config = AgentConfig {
                session_id: "agent-1".to_string(),
                session_name: "SavedName".to_string(),
                agent_class: "Coder".to_string(),
                folder: workspace.to_string_lossy().to_string(),
                provider: "codex".to_string(),
                resume_session: Some("saved-provider-session".to_string()),
                model: Some("saved-model".to_string()),
                ..Default::default()
            };
        }
        state
            .agents
            .lock()
            .await
            .insert("agent-1".to_string(), agent);

        let command = build_agent_cli_command_for_session_id_with_shells(
            "agent-1", &state, &settings, &shells,
        )
        .await
        .expect("full command");

        assert!(command.contains("saved-provider-session"));
        assert!(command.contains("saved-model"));
        assert!(!command.contains("draft-model"));
    }

    #[test]
    fn full_agent_command_rejects_malformed_custom_args() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let (settings, shells) = test_pwsh_shell();

        let err = build_agent_cli_command_with_shells(
            &AgentConfig {
                session_id: "agent-1".to_string(),
                session_name: "CoderOne".to_string(),
                agent_class: "Coder".to_string(),
                folder: workspace.to_string_lossy().to_string(),
                provider: "claude".to_string(),
                resume_session: Some("claude-session".to_string()),
                custom_args: Some("--flag 'unterminated".to_string()),
                ..Default::default()
            },
            &settings,
            &shells,
        )
        .expect_err("malformed custom args must fail");

        assert_eq!(err, "Provider custom arguments are not valid shell syntax");
    }

    #[test]
    fn full_agent_command_strips_claude_embedded_stream_flags() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let (settings, shells) = test_pwsh_shell();

        let command = build_agent_cli_command_with_shells(
            &AgentConfig {
                session_id: "agent-1".to_string(),
                session_name: "ClaudeOne".to_string(),
                agent_class: "Coder".to_string(),
                folder: workspace.to_string_lossy().to_string(),
                provider: "claude".to_string(),
                resume_session: Some("claude-session".to_string()),
                model: Some("sonnet".to_string()),
                ..Default::default()
            },
            &settings,
            &shells,
        )
        .expect("full command");

        assert!(command.contains("--resume"));
        assert!(command.contains("claude-session"));
        assert!(command.contains("--model"));
        assert!(command.contains("sonnet"));
        assert!(!command.contains("stream-json"));
        assert!(!command.contains("--verbose"));
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
        assert_eq!(
            preview.default_selected_files,
            vec!["AGENTS.md".to_string()]
        );
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
        std::fs::create_dir_all(source_root.join("claude")).expect("claude");
        std::fs::create_dir_all(source_root.join("logs")).expect("logs");
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
        std::fs::write(
            source_root.join("claude").join("permission-requests.jsonl"),
            "{}\n",
        )
        .expect("permission log");
        std::fs::write(source_root.join("logs").join("agent.log"), "log").expect("log");
        std::fs::write(source_root.join("transcript.jsonl"), "{}").expect("transcript");

        let files = clone_collect_eligible_file_tree(home, "source-agent").expect("files");
        let paths = flatten_clone_file_paths(&files);

        assert!(paths.contains(&"AGENTS.md".to_string()));
        assert!(paths.contains(&"nested/keep.md".to_string()));
        assert!(!paths.iter().any(|path| path.starts_with("habitat/")));
        assert!(!paths.iter().any(|path| path.starts_with(".agents/skills/")));
        assert!(!paths.iter().any(|path| path.starts_with(".codex/")));
        assert!(!paths.iter().any(|path| path.starts_with("claude/")));
        assert!(!paths.iter().any(|path| path.starts_with("logs/")));
        assert!(!paths.iter().any(|path| path.ends_with(".jsonl")));
        assert!(!paths.iter().any(|path| path.ends_with(".log")));
    }

    #[test]
    fn clone_selected_profile_file_rejects_runtime_artifacts() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        std::fs::create_dir_all(source.join("claude")).expect("claude");
        std::fs::write(source.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(
            source.join("claude").join("permission-requests.jsonl"),
            "{}\n",
        )
        .expect("permission log");

        assert!(clone_validate_selected_profile_files(
            home,
            "source-agent",
            &["claude/permission-requests.jsonl".to_string()]
        )
        .is_err());
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
    fn clone_custom_profile_copy_copies_only_selected_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        let dest = home.join("agents").join("dest-agent");
        std::fs::create_dir_all(source.join("nested")).expect("nested");
        std::fs::write(source.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(source.join("notes.md"), "notes").expect("notes");
        std::fs::write(source.join("nested").join("keep.md"), "keep").expect("keep");

        clone_copy_selected_agent_profile_files(
            home,
            "source-agent",
            "dest-agent",
            &["AGENTS.md".to_string(), "nested/keep.md".to_string()],
        )
        .expect("copy selected");

        assert!(dest.join("AGENTS.md").is_file());
        assert!(dest.join("nested").join("keep.md").is_file());
        assert!(!dest.join("notes.md").exists());
    }

    #[test]
    fn clone_selected_profile_file_rejects_traversal_and_absolute_paths() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::write(source.join("AGENTS.md"), "# Agent\n").expect("agents");

        assert!(clone_validate_selected_profile_files(
            home,
            "source-agent",
            &["../secret.md".to_string()]
        )
        .is_err());
        assert!(clone_validate_selected_profile_files(
            home,
            "source-agent",
            &["C:/secret.md".to_string()]
        )
        .is_err());
    }

    #[test]
    fn clone_selected_profile_file_rejects_links_and_escaped_canonical_targets() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        let external = home.join("external");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::create_dir_all(&external).expect("external");
        std::fs::write(source.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(external.join("secret.md"), "secret").expect("secret");

        let linked = source.join("linked");
        if create_directory_link(&external, &linked).is_err() {
            return;
        }

        assert!(clone_validate_selected_profile_files(
            home,
            "source-agent",
            &["linked/secret.md".to_string()]
        )
        .is_err());
    }

    #[test]
    fn clone_custom_profile_copy_recreates_only_selected_skills() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;

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
        .expect("deploy selected");

        let ignored_skill = temp.path().join("library").join("skills").join("ignored");
        std::fs::create_dir_all(&ignored_skill).expect("ignored");
        std::fs::write(ignored_skill.join("SKILL.md"), "ignored").expect("ignored skill");
        crate::commands::library::deploy_skill_from_library("ignored", "agent", "source-agent")
            .expect("deploy ignored");

        clone_copy_selected_agent_skills(
            temp.path(),
            "source-agent",
            "dest-agent",
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-a/planner".to_string()),
            }],
        )
        .expect("copy selected skills");

        assert!(temp
            .path()
            .join("agents/dest-agent/.agents/skills/planner/SKILL.md")
            .is_file());
        assert!(!temp
            .path()
            .join("agents/dest-agent/.agents/skills/ignored")
            .exists());
    }

    #[test]
    fn clone_custom_profile_copy_supports_legacy_copied_skills() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;
        let legacy = temp
            .path()
            .join("agents/source-agent/.agents/skills/legacy");
        std::fs::create_dir_all(&legacy).expect("legacy skill");
        std::fs::write(legacy.join("SKILL.md"), "legacy").expect("legacy skill file");

        clone_copy_selected_agent_skills(
            temp.path(),
            "source-agent",
            "dest-agent",
            &[DeployedSkillRef {
                name: "legacy".to_string(),
                source_path: None,
            }],
        )
        .expect("copy legacy skill");

        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join("agents/dest-agent/.agents/skills/legacy/SKILL.md")
            )
            .expect("legacy copied"),
            "legacy"
        );
    }

    #[test]
    fn clone_custom_profile_copy_preserves_unmarked_legacy_skill_with_same_named_library_skill() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;

        let library = temp.path().join("library/skills/group-a/planner");
        std::fs::create_dir_all(&library).expect("library skill");
        std::fs::write(library.join("SKILL.md"), "library planner").expect("library skill file");
        let legacy = temp
            .path()
            .join("agents/source-agent/.agents/skills/planner");
        std::fs::create_dir_all(&legacy).expect("legacy skill");
        std::fs::write(legacy.join("SKILL.md"), "customized planner").expect("legacy skill file");

        clone_copy_selected_agent_skills(
            temp.path(),
            "source-agent",
            "dest-agent",
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: None,
            }],
        )
        .expect("copy legacy skill");

        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join("agents/dest-agent/.agents/skills/planner/SKILL.md")
            )
            .expect("legacy copied"),
            "customized planner"
        );
    }

    #[test]
    fn clone_custom_skill_selection_matches_duplicate_names_by_source_path() {
        let matched = clone_match_selected_agent_skills(
            &[
                DeployedSkillRef {
                    name: "planner".to_string(),
                    source_path: Some("group-a/planner".to_string()),
                },
                DeployedSkillRef {
                    name: "planner".to_string(),
                    source_path: Some("group-b/planner".to_string()),
                },
                DeployedSkillRef {
                    name: "planner".to_string(),
                    source_path: None,
                },
            ],
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-b/planner".to_string()),
            }],
        )
        .expect("match duplicate");

        assert_eq!(
            matched,
            vec![DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-b/planner".to_string()),
            }]
        );
    }

    #[test]
    fn clone_custom_skill_selection_rejects_mismatched_duplicate_refs() {
        let err = clone_match_selected_agent_skills(
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-a/planner".to_string()),
            }],
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-b/planner".to_string()),
            }],
        )
        .expect_err("mismatched source_path must fail");

        assert!(err.contains("not deployed"));
    }

    #[test]
    fn clone_custom_skill_selection_none_source_path_matches_only_legacy_ref() {
        let matched = clone_match_selected_agent_skills(
            &[
                DeployedSkillRef {
                    name: "planner".to_string(),
                    source_path: Some("group-a/planner".to_string()),
                },
                DeployedSkillRef {
                    name: "planner".to_string(),
                    source_path: None,
                },
            ],
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: None,
            }],
        )
        .expect("match legacy");

        assert_eq!(
            matched,
            vec![DeployedSkillRef {
                name: "planner".to_string(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn clone_custom_skill_selection_rejects_missing_source_backed_library_skill() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;
        let deployed = temp
            .path()
            .join("agents/source-agent/.agents/skills/planner");
        std::fs::create_dir_all(&deployed).expect("deployed skill");
        std::fs::write(deployed.join("SKILL.md"), "stale").expect("skill");
        std::fs::write(deployed.join(".wardian-skill-source"), "group-a/planner").expect("marker");

        let err = clone_validate_selected_agent_skills(
            "source-agent",
            &[DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-a/planner".to_string()),
            }],
        )
        .expect_err("missing library source fails");

        assert!(err.contains("not deployed") || err.contains("Skill source not found"));
    }

    #[test]
    fn clone_profile_copy_plan_custom_copies_selected_files_and_skills() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;
        let source = temp.path().join("agents").join("source-agent");
        std::fs::create_dir_all(source.join("nested")).expect("nested");
        std::fs::write(source.join("AGENTS.md"), "# Agent\n").expect("agents");
        std::fs::write(source.join("notes.md"), "notes").expect("notes");
        std::fs::write(source.join("nested").join("keep.md"), "keep").expect("keep");
        let legacy = source.join(".agents/skills/legacy");
        std::fs::create_dir_all(&legacy).expect("legacy");
        std::fs::write(legacy.join("SKILL.md"), "legacy").expect("legacy skill");

        let selection = CloneProfileSelection {
            files: vec!["nested/keep.md".to_string()],
            skills: vec![DeployedSkillRef {
                name: "legacy".to_string(),
                source_path: None,
            }],
        };

        clone_copy_profile_plan(
            temp.path(),
            "source-agent",
            "dest-agent",
            CloneProfileCopyPlan::Custom(&selection),
        )
        .expect("copy custom profile");

        let dest = temp.path().join("agents").join("dest-agent");
        assert!(dest.join("nested/keep.md").is_file());
        assert!(!dest.join("AGENTS.md").exists());
        assert!(dest.join(".agents/skills/legacy/SKILL.md").is_file());
    }

    #[test]
    fn clone_cleanup_created_profile_dirs_removes_only_tracked_clone_dirs() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let provisional = home.join("agents").join("provisional-agent");
        let final_dir = home.join("agents").join("final-agent");
        let unrelated = home.join("agents").join("unrelated-agent");
        std::fs::create_dir_all(&provisional).expect("provisional");
        std::fs::create_dir_all(&final_dir).expect("final");
        std::fs::create_dir_all(&unrelated).expect("unrelated");
        std::fs::write(provisional.join("AGENTS.md"), "provisional").expect("provisional file");
        std::fs::write(final_dir.join("AGENTS.md"), "final").expect("final file");
        std::fs::write(unrelated.join("AGENTS.md"), "unrelated").expect("unrelated file");

        clone_cleanup_created_profile_dirs(&[provisional.clone(), final_dir.clone()]);

        assert!(!provisional.exists());
        assert!(!final_dir.exists());
        assert!(unrelated.join("AGENTS.md").is_file());
    }

    #[test]
    fn clone_custom_discovered_session_final_profile_is_copied_from_source_not_provisional() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let source = home.join("agents").join("source-agent");
        let provisional = home.join("agents").join("provisional-agent");
        let final_dir = home.join("agents").join("real-provider-session");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::write(source.join("AGENTS.md"), "source profile").expect("source profile");

        let selection = CloneProfileSelection {
            files: vec!["AGENTS.md".to_string()],
            skills: Vec::new(),
        };

        clone_copy_profile_plan(
            home,
            "source-agent",
            "provisional-agent",
            CloneProfileCopyPlan::Custom(&selection),
        )
        .expect("copy provisional");
        std::fs::write(
            provisional.join("AGENTS.md"),
            "provider mutated provisional",
        )
        .expect("mutate provisional");

        clone_copy_profile_plan(
            home,
            "source-agent",
            "real-provider-session",
            CloneProfileCopyPlan::Custom(&selection),
        )
        .expect("copy final");
        clone_remove_existing_path(&provisional);

        assert!(!provisional.exists());
        assert_eq!(
            std::fs::read_to_string(final_dir.join("AGENTS.md")).expect("final profile"),
            "source profile"
        );
    }

    #[test]
    fn clone_custom_discovered_session_refreshes_system_includes_to_final_session_id() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _guard = WardianHomeGuard;
        let mut config = AgentConfig {
            agent_class: "Coder".to_string(),
            system_include_directories: Some(crate::utils::fs::resolve_system_include_directories(
                "Coder",
                "provisional-agent",
            )),
            ..Default::default()
        };

        clone_refresh_profile_system_include_directories(
            &mut config,
            "real-provider-session",
            true,
        );

        let joined = config
            .system_include_directories
            .expect("include dirs")
            .join("|")
            .replace('\\', "/");
        assert!(joined.contains("/agents/real-provider-session"));
        assert!(!joined.contains("/agents/provisional-agent"));
    }

    #[test]
    fn clone_folder_override_uses_spawn_workspace_validation() {
        let temp = tempfile::tempdir().expect("temp dir");
        let valid =
            normalize_clone_folder_override(Some(temp.path().to_string_lossy().to_string()))
                .expect("valid folder")
                .expect("folder override");
        assert_eq!(
            valid,
            normalize_spawn_folder(&temp.path().to_string_lossy()).unwrap()
        );

        let missing = temp.path().join("missing");
        let err = normalize_clone_folder_override(Some(missing.to_string_lossy().to_string()))
            .expect_err("missing workspace should fail");
        assert!(err.contains("Path does not exist or is invalid"));
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

    #[test]
    fn resolve_agent_worktree_branch_name_slugifies_session_name() {
        assert_eq!(
            resolve_agent_worktree_branch_name("Repo Agent!! 2"),
            "wardian/repo-agent-2"
        );
        assert_eq!(resolve_agent_worktree_branch_name(" !!! "), "wardian/agent");
    }

    #[test]
    fn resolve_agent_worktree_path_uses_session_name_when_worktree_name_is_missing() {
        let home = std::path::Path::new("C:/wardian");

        assert_eq!(
            resolve_agent_worktree_path(home, "agent-1", None, "Repo Agent"),
            std::path::PathBuf::from("C:/wardian")
                .join("agents")
                .join("agent-1")
                .join("worktrees")
                .join("repo-agent")
        );
    }

    #[test]
    fn resolve_agent_worktree_path_uses_requested_worktree_name() {
        let home = std::path::Path::new("C:/wardian");

        assert_eq!(
            resolve_agent_worktree_path(home, "agent-1", Some("Review Fixes"), "Repo Agent"),
            std::path::PathBuf::from("C:/wardian")
                .join("agents")
                .join("agent-1")
                .join("worktrees")
                .join("review-fixes")
        );
    }

    #[test]
    fn enable_worktree_config_records_worktree_and_moves_launch_folder_to_worktree() {
        let worktree_path = std::path::Path::new("C:/wardian/agents/agent-1/worktree");
        let mut config = AgentConfig {
            folder: "C:/repo".to_string(),
            ..Default::default()
        };

        enable_worktree_config(&mut config, worktree_path);

        assert_eq!(config.git_worktree, Some(true));
        assert_eq!(config.git_worktree_source.as_deref(), Some("C:/repo"));
        let expected_worktree = worktree_path.to_string_lossy().replace('\\', "/");
        assert_eq!(config.folder, expected_worktree);
        assert_eq!(
            config.git_worktree_folder.as_deref(),
            Some(expected_worktree.as_str())
        );
    }

    #[test]
    fn enable_worktree_config_preserves_existing_source() {
        let worktree_path = std::path::Path::new("C:/wardian/agents/agent-1/worktree");
        let mut config = AgentConfig {
            folder: "C:/wardian/agents/agent-1/worktree".to_string(),
            git_worktree_source: Some("C:/repo".to_string()),
            ..Default::default()
        };

        enable_worktree_config(&mut config, worktree_path);

        assert_eq!(config.git_worktree_source.as_deref(), Some("C:/repo"));
    }

    #[test]
    fn disable_worktree_config_clears_worktree_and_restores_source_launch_folder() {
        let mut config = AgentConfig {
            folder: "C:/wardian/agents/agent-1/worktree".to_string(),
            git_worktree: Some(true),
            git_worktree_source: Some("C:/repo".to_string()),
            git_worktree_folder: Some("C:/wardian/agents/agent-1/worktree".to_string()),
            ..Default::default()
        };

        disable_worktree_config(&mut config).expect("disable worktree");

        assert_eq!(config.git_worktree, Some(false));
        assert_eq!(config.git_worktree_source, None);
        assert_eq!(config.git_worktree_folder, None);
        assert_eq!(config.folder, "C:/repo");
    }

    #[test]
    fn disable_worktree_config_requires_original_source() {
        let mut config = AgentConfig {
            folder: "C:/wardian/agents/agent-1/worktree".to_string(),
            git_worktree: Some(true),
            ..Default::default()
        };

        let err = disable_worktree_config(&mut config).expect_err("missing source should fail");

        assert!(err.contains("original workspace"));
        assert_eq!(config.git_worktree, Some(true));
    }

    #[test]
    fn collect_agent_worktrees_groups_members_by_worktree_folder() {
        let configs = vec![
            AgentConfig {
                session_id: "agent-1".to_string(),
                session_name: "agent-one".to_string(),
                folder: "C:/repo".to_string(),
                git_worktree: Some(true),
                git_worktree_source: Some("C:/repo".to_string()),
                git_worktree_folder: Some("C:/repo-worktree".to_string()),
                ..Default::default()
            },
            AgentConfig {
                session_id: "agent-2".to_string(),
                session_name: "agent-two".to_string(),
                folder: "C:/repo".to_string(),
                git_worktree: Some(true),
                git_worktree_source: Some("C:/repo".to_string()),
                git_worktree_folder: Some("C:\\repo-worktree".to_string()),
                ..Default::default()
            },
            AgentConfig {
                session_id: "agent-3".to_string(),
                folder: "C:/other".to_string(),
                ..Default::default()
            },
        ];

        let worktrees = collect_agent_worktrees(&configs);

        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].name, "repo-worktree");
        assert_eq!(worktrees[0].worktree_folder, "C:/repo-worktree");
        assert_eq!(worktrees[0].source_folder, "C:/repo");
        assert_eq!(
            worktrees[0].member_agent_ids,
            vec!["agent-1".to_string(), "agent-2".to_string()]
        );
    }

    #[test]
    fn collect_agent_worktrees_includes_unassigned_git_worktrees_under_agent_roots() {
        let home = tempfile::tempdir().expect("home");
        let wardian_home = home.path();
        let discovered_path = wardian_home
            .join("agents")
            .join("agent-1")
            .join("worktrees")
            .join("manual-review");
        std::fs::create_dir_all(&discovered_path).expect("create discovered worktree");

        let configs = vec![AgentConfig {
            session_id: "agent-1".to_string(),
            session_name: "agent-one".to_string(),
            folder: "/repo".to_string(),
            ..Default::default()
        }];
        let discovered = vec![DiscoveredGitWorktree {
            source_folder: "/repo".to_string(),
            worktree_folder: normalize_workspace_record_path(&discovered_path),
        }];

        let worktrees = collect_agent_worktrees_with_discovered(&configs, wardian_home, discovered);

        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].name, "manual-review");
        assert_eq!(worktrees[0].source_folder, "/repo");
        assert_eq!(
            worktrees[0].worktree_folder,
            normalize_existing_workspace_record_path(&discovered_path)
        );
        assert!(worktrees[0].member_agent_ids.is_empty());
        assert!(worktrees[0].can_delete);
    }

    #[test]
    fn collect_agent_worktrees_includes_external_git_worktrees_without_delete_capability() {
        let home = tempfile::tempdir().expect("home");
        let outside = home.path().join("outside-review");
        std::fs::create_dir_all(&outside).expect("create outside worktree");

        let configs = vec![AgentConfig {
            session_id: "agent-1".to_string(),
            folder: "/repo".to_string(),
            ..Default::default()
        }];
        let discovered = vec![DiscoveredGitWorktree {
            source_folder: "/repo".to_string(),
            worktree_folder: normalize_workspace_record_path(&outside),
        }];

        let worktrees = collect_agent_worktrees_with_discovered(&configs, home.path(), discovered);

        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].name, "outside-review");
        assert_eq!(
            worktrees[0].worktree_folder,
            normalize_existing_workspace_record_path(&outside)
        );
        assert!(worktrees[0].member_agent_ids.is_empty());
        assert!(!worktrees[0].can_delete);
    }

    #[test]
    fn agent_worktree_root_match_handles_windows_verbatim_home_prefix() {
        let home = std::path::Path::new(r"\\?\D:\a\Wardian\wardian-home");
        let worktree = "D:/a/Wardian/wardian-home/agents/agent-1/worktrees/manual-review";

        assert!(is_under_wardian_agent_worktree_root(home, worktree));
    }

    #[test]
    fn agent_worktree_root_match_handles_windows_msys_worktree_path() {
        let home = std::path::Path::new(r"D:\a\Wardian\wardian-home");
        let worktree = "/d/a/Wardian/wardian-home/agents/agent-1/worktrees/manual-review";

        if cfg!(windows) {
            assert!(is_under_wardian_agent_worktree_root(home, worktree));
        } else {
            assert!(!is_under_wardian_agent_worktree_root(home, worktree));
        }
    }

    #[test]
    fn workspace_path_match_handles_windows_source_variants() {
        let left = r"\\?\D:\a\Wardian\repo";
        let right = "d:/a/Wardian/repo/";

        if cfg!(windows) {
            assert!(workspace_paths_match(left, right));
        } else {
            assert!(!workspace_paths_match(left, right));
        }
    }

    #[test]
    fn discover_git_worktrees_reads_git_registry_for_known_sources() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");
        let cwd = repo.to_str().unwrap();
        crate::commands::git::run_git(cwd, &["init"]).expect("git init");
        crate::commands::git::run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        crate::commands::git::run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(repo.join("README.md"), "initial\n").expect("readme");
        crate::commands::git::run_git(cwd, &["add", "README.md"]).unwrap();
        crate::commands::git::run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let wardian_home = temp.path().join("wardian-home");
        let worktree = wardian_home
            .join("agents")
            .join("agent-1")
            .join("worktrees")
            .join("manual-review");
        let external_worktree = temp.path().join("external-review");
        crate::commands::git::create_worktree_with_build_caches(
            &repo,
            &worktree,
            "feat/manual-review",
        )
        .expect("create git worktree");
        crate::commands::git::create_worktree_with_build_caches(
            &repo,
            &external_worktree,
            "feat/external-review",
        )
        .expect("create external git worktree");

        let configs = vec![AgentConfig {
            session_id: "agent-1".to_string(),
            folder: normalize_workspace_record_path(&repo),
            ..Default::default()
        }];

        let discovered = discover_git_worktrees_for_configs(&configs, &wardian_home);

        let raw_worktree_list =
            crate::commands::git::run_git(cwd, &["worktree", "list", "--porcelain"])
                .unwrap_or_else(|error| format!("<git worktree list failed: {error}>"));
        let listed_worktrees = crate::commands::git::list_git_worktrees(&repo)
            .map(|worktrees| {
                worktrees
                    .into_iter()
                    .map(|worktree| {
                        let normalized = normalize_discovered_git_worktree_path(&worktree.path);
                        format!(
                            "path={:?}, normalized={normalized:?}, under_root={}",
                            worktree.path,
                            is_under_wardian_agent_worktree_root(&wardian_home, &normalized)
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|error| vec![format!("<list_git_worktrees failed: {error}>")]);

        assert_eq!(
            discovered.len(),
            2,
            "repo={}, wardian_home={}, worktree={}, external_worktree={}, raw_worktree_list={raw_worktree_list:?}, listed_worktrees={listed_worktrees:#?}",
            normalize_workspace_record_path(&repo),
            normalize_workspace_record_path(&wardian_home),
            normalize_workspace_record_path(&worktree),
            normalize_workspace_record_path(&external_worktree)
        );
        assert!(discovered
            .iter()
            .all(|entry| entry.source_folder == normalize_existing_workspace_record_path(&repo)));
        assert!(discovered.iter().any(
            |entry| entry.worktree_folder == normalize_discovered_git_worktree_path(&worktree)
        ));
        assert!(discovered.iter().any(|entry| entry.worktree_folder
            == normalize_discovered_git_worktree_path(&external_worktree)));
        assert!(!discovered.iter().any(|entry| workspace_paths_match(
            &entry.worktree_folder,
            &normalize_workspace_record_path(&repo)
        )));
    }

    #[test]
    fn existing_non_git_worktree_folder_is_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = temp.path().join("repo");
        let existing = temp
            .path()
            .join("wardian-home")
            .join("agents")
            .join("agent-1")
            .join("worktrees")
            .join("review");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&existing).unwrap();
        let cwd = repo.to_str().unwrap();
        crate::commands::git::run_git(cwd, &["init"]).unwrap();

        let err = ensure_existing_worktree_is_git_registered(&repo, &existing)
            .expect_err("existing non-git worktree should be rejected");

        assert!(err.contains("not registered with Git"));
    }

    #[test]
    fn assignable_worktree_validation_rejects_stale_non_git_assignment() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repo = temp.path().join("repo");
        let stale_worktree = temp
            .path()
            .join("wardian-home")
            .join("agents")
            .join("agent-1")
            .join("worktrees")
            .join("manual-review");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&stale_worktree).unwrap();
        let cwd = repo.to_str().unwrap();
        crate::commands::git::run_git(cwd, &["init"]).unwrap();

        let managed_worktree = AgentWorktreeSummary {
            id: normalize_existing_workspace_record_path(&stale_worktree),
            name: "manual-review".to_string(),
            source_folder: normalize_existing_workspace_record_path(&repo),
            worktree_folder: normalize_existing_workspace_record_path(&stale_worktree),
            member_agent_ids: vec!["agent-2".to_string()],
            can_delete: false,
        };

        let err = validate_assignable_worktree_for_agent(
            &normalize_workspace_record_path(&repo),
            &managed_worktree,
            &stale_worktree,
        )
        .expect_err("stale non-git assignment should not be assignable");

        assert!(err.contains("not registered with Git"));
    }

    #[test]
    fn deletable_worktree_validation_rejects_assigned_worktree() {
        let home = tempfile::tempdir().expect("home");
        let managed_worktree = AgentWorktreeSummary {
            id: normalize_workspace_record_path(
                &home
                    .path()
                    .join("agents")
                    .join("agent-1")
                    .join("worktrees")
                    .join("review"),
            ),
            name: "review".to_string(),
            source_folder: normalize_workspace_record_path(std::path::Path::new("C:/repo")),
            worktree_folder: normalize_workspace_record_path(
                &home
                    .path()
                    .join("agents")
                    .join("agent-1")
                    .join("worktrees")
                    .join("review"),
            ),
            member_agent_ids: vec!["agent-1".to_string()],
            can_delete: false,
        };

        let err = validate_deletable_agent_worktree(home.path(), &managed_worktree)
            .expect_err("assigned worktree should not be deletable");

        assert!(err.contains("assigned"));
    }

    #[test]
    fn deletable_worktree_validation_rejects_paths_outside_wardian_root() {
        let home = tempfile::tempdir().expect("home");
        let outside = tempfile::tempdir().expect("outside");
        let managed_worktree = AgentWorktreeSummary {
            id: normalize_workspace_record_path(outside.path()),
            name: "outside".to_string(),
            source_folder: normalize_workspace_record_path(std::path::Path::new("C:/repo")),
            worktree_folder: normalize_workspace_record_path(outside.path()),
            member_agent_ids: Vec::new(),
            can_delete: false,
        };

        let err = validate_deletable_agent_worktree(home.path(), &managed_worktree)
            .expect_err("outside worktree should not be deletable");

        assert!(err.contains("Wardian agent worktree"));
    }

    #[test]
    fn deletable_worktree_validation_rejects_source_checkout_path() {
        let home = tempfile::tempdir().expect("home");
        let source = home
            .path()
            .join("agents")
            .join("agent-1")
            .join("worktrees")
            .join("review");
        std::fs::create_dir_all(&source).expect("source path");
        let source_folder = normalize_workspace_record_path(&source);
        let managed_worktree = AgentWorktreeSummary {
            id: source_folder.clone(),
            name: "review".to_string(),
            source_folder: source_folder.clone(),
            worktree_folder: source_folder,
            member_agent_ids: Vec::new(),
            can_delete: true,
        };

        let err = validate_deletable_agent_worktree(home.path(), &managed_worktree)
            .expect_err("source checkout should not be deletable as a worktree");

        assert!(err.contains("source checkout"));
    }

    #[test]
    fn find_assignable_worktree_matches_discovered_unassigned_worktree() {
        let home = tempfile::tempdir().expect("home");
        let worktree = home
            .path()
            .join("agents")
            .join("agent-1")
            .join("worktrees")
            .join("manual-review");
        std::fs::create_dir_all(&worktree).unwrap();
        let worktree_folder = normalize_workspace_record_path(&worktree);
        let configs = vec![AgentConfig {
            session_id: "agent-1".to_string(),
            folder: "/repo".to_string(),
            ..Default::default()
        }];
        let discovered = vec![DiscoveredGitWorktree {
            source_folder: "/repo".to_string(),
            worktree_folder: worktree_folder.clone(),
        }];

        let found = find_assignable_worktree(&configs, home.path(), &worktree_folder, discovered)
            .expect("discovered worktree should be assignable");

        assert_eq!(found.name, "manual-review");
        assert!(found.member_agent_ids.is_empty());
        assert!(found.can_delete);
    }

    #[test]
    fn find_assignable_worktree_matches_external_discovered_worktree_but_marks_not_deletable() {
        let home = tempfile::tempdir().expect("home");
        let outside = tempfile::tempdir().expect("outside");
        let worktree_folder = normalize_workspace_record_path(outside.path());
        let configs = vec![AgentConfig {
            session_id: "agent-1".to_string(),
            folder: "/repo".to_string(),
            ..Default::default()
        }];
        let discovered = vec![DiscoveredGitWorktree {
            source_folder: "/repo".to_string(),
            worktree_folder: worktree_folder.clone(),
        }];

        let found = find_assignable_worktree(&configs, home.path(), &worktree_folder, discovered)
            .expect("external discovered worktree should be assignable");

        assert_eq!(
            found.name,
            outside.path().file_name().unwrap().to_string_lossy()
        );
        assert!(found.member_agent_ids.is_empty());
        assert!(!found.can_delete);
    }

    #[test]
    fn assign_worktree_config_records_shared_worktree_and_moves_launch_folder() {
        let mut config = AgentConfig {
            folder: "C:/repo".to_string(),
            ..Default::default()
        };

        assign_worktree_config(&mut config, "C:\\repo-worktree").expect("assign worktree");

        assert_eq!(config.folder, "C:/repo-worktree");
        assert_eq!(config.git_worktree, Some(true));
        assert_eq!(config.git_worktree_source.as_deref(), Some("C:/repo"));
        assert_eq!(
            config.git_worktree_folder.as_deref(),
            Some("C:/repo-worktree")
        );
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
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                cleared_provider_sessions: vec!["old-provider-session".to_string()],
                ..Default::default()
            }),
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
        assert!(clone.codex_config().cleared_provider_sessions.is_empty());
        assert_eq!(clone.opencode_port, None);
        assert!(!clone.is_off);
    }

    #[test]
    fn clone_sanitize_config_preserves_same_provider_claude_config() {
        let source = AgentConfig {
            provider: "claude".to_string(),
            provider_config: ProviderConfig::Claude(ClaudeProviderConfig {
                permission_mode: Some("plan".to_string()),
                ..Default::default()
            }),
            resume_session: Some("old-session".to_string()),
            ..Default::default()
        };

        let clone =
            clone_sanitize_config(&source, "Alpha-copy".to_string(), None, None, None, true);

        assert_eq!(clone.provider, "claude");
        assert_eq!(
            clone.claude_config().permission_mode.as_deref(),
            Some("plan")
        );
        assert!(matches!(clone.provider_config, ProviderConfig::Claude(_)));
    }

    #[test]
    fn clone_sanitize_config_preserves_same_provider_gemini_config() {
        let source = AgentConfig {
            provider: "gemini".to_string(),
            provider_config: ProviderConfig::Gemini(GeminiProviderConfig {
                sandbox: Some(true),
                ..Default::default()
            }),
            resume_session: Some("old-session".to_string()),
            ..Default::default()
        };

        let clone =
            clone_sanitize_config(&source, "Alpha-copy".to_string(), None, None, None, true);

        assert_eq!(clone.provider, "gemini");
        assert_eq!(clone.gemini_config().sandbox, Some(true));
        assert!(matches!(clone.provider_config, ProviderConfig::Gemini(_)));
    }

    #[test]
    fn clone_sanitize_config_preserves_same_provider_opencode_config_and_clears_port() {
        let source = AgentConfig {
            provider: "opencode".to_string(),
            provider_config: ProviderConfig::OpenCode(OpenCodeProviderConfig {
                agent: Some("build".to_string()),
                port: Some(4096),
            }),
            resume_session: Some("ses_old".to_string()),
            ..Default::default()
        };

        let clone =
            clone_sanitize_config(&source, "Alpha-copy".to_string(), None, None, None, true);

        assert_eq!(clone.provider, "opencode");
        let opencode = clone.opencode_config();
        assert_eq!(opencode.agent.as_deref(), Some("build"));
        assert_eq!(opencode.port, None);
        assert!(matches!(clone.provider_config, ProviderConfig::OpenCode(_)));
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
    fn opencode_uses_generated_wardian_id_until_visible_pty_reports_provider_session() {
        assert!(provider_uses_generated_session_id("opencode"));
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
    fn gemini_uses_generated_wardian_id_until_visible_pty_reports_provider_session() {
        assert!(provider_uses_generated_session_id("gemini"));
    }

    #[test]
    fn antigravity_uses_generated_wardian_id_until_transcript_reports_provider_session() {
        assert!(provider_uses_generated_session_id("antigravity"));
    }

    #[test]
    fn gemini_clear_starts_visible_fresh_session_without_headless_bootstrap() {
        assert!(!provider_needs_obtain_session_id_on_clear("gemini"));
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
    fn manual_session_provider_resume_promotes_fresh_provider_session_to_resume_session() {
        let mut new_active = make_test_agent();
        {
            let mut config = new_active.config.lock().unwrap();
            config.fresh_provider_session_id = Some("gemini-fresh-session".to_string());
            config.resume_session = None;
        }
        *new_active.log_path.lock().unwrap() = Some(std::path::PathBuf::from("C:/tmp/old.jsonl"));
        *new_active.log_last_modified.lock().unwrap() = Some(std::time::SystemTime::now());

        promote_fresh_provider_session_after_resume("gemini", &mut new_active);

        let config = new_active.config.lock().unwrap();
        assert_eq!(
            config.resume_session.as_deref(),
            Some("gemini-fresh-session")
        );
        assert_eq!(config.fresh_provider_session_id, None);
        assert_eq!(*new_active.log_path.lock().unwrap(), None);
        assert_eq!(*new_active.log_last_modified.lock().unwrap(), None);
    }

    #[test]
    fn non_manual_session_provider_resume_keeps_fresh_provider_session_field() {
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
    fn detach_agent_for_kill_removes_live_state_and_input_sender() {
        let mut agents = std::collections::HashMap::new();
        let mut order = vec!["agent-1".to_string(), "agent-2".to_string()];
        let input_senders = std::sync::RwLock::new(std::collections::HashMap::new());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);
        let agent = make_test_agent();
        agent.config.lock().unwrap().session_id = "agent-1".to_string();
        agents.insert("agent-1".to_string(), agent);
        agents.insert("agent-2".to_string(), make_test_agent());

        let detached = detach_agent_for_kill(&mut agents, &mut order, &input_senders, "agent-1")
            .expect("agent should be detached");

        assert_eq!(detached.config.lock().unwrap().session_id, "agent-1");
        assert!(!agents.contains_key("agent-1"));
        assert!(agents.contains_key("agent-2"));
        assert_eq!(order, vec!["agent-2".to_string()]);
        assert!(!input_senders.read().unwrap().contains_key("agent-1"));
    }

    #[tokio::test]
    async fn lock_agent_lifecycle_serializes_same_session() {
        let state = Arc::new(AppState::new());
        let first_guard = lock_agent_lifecycle(&state, "agent-1").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let state_for_task = Arc::clone(&state);

        let waiter = tokio::spawn(async move {
            let _second_guard = lock_agent_lifecycle(&state_for_task, "agent-1").await;
            tx.send(()).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(rx.try_recv().is_err());

        drop(first_guard);
        rx.recv()
            .await
            .expect("second lifecycle lock should acquire");
        waiter.await.unwrap();
    }

    #[test]
    fn take_agent_runtime_for_termination_detaches_process_related_state() {
        let mut active = make_test_agent();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        active.stdin_tx = Some(tx);
        active.process_id = Some(12345);
        active.pty_master = None;

        let detached = take_agent_runtime_for_termination(&mut active);

        assert_eq!(detached.process_id, Some(12345));
        assert!(detached.stdin_tx.is_some());
        assert_eq!(active.process_id, None);
        assert!(active.stdin_tx.is_none());
        assert!(active.child_process.is_none());
        assert!(active.background_processes.is_empty());
    }

    #[test]
    fn prepare_agent_for_clear_resets_visible_runtime_without_terminating_inline() {
        let mut active = make_test_agent();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        active.stdin_tx = Some(tx);
        active.process_id = Some(12345);
        let runtime_status = active.current_status.clone();
        active.output_buffer.lock().unwrap().push_str("old output");
        *active.terminal_title.lock().unwrap() = "Old Title".to_string();
        *active.current_status.lock().unwrap() = "Idle".to_string();
        *active.query_count.lock().unwrap() = 5;
        *active.log_path.lock().unwrap() = Some(std::path::PathBuf::from("D:/tmp/agent.log"));
        *active.log_last_modified.lock().unwrap() = Some(std::time::SystemTime::now());
        *active.init_timestamp.lock().unwrap() = Some("2026-05-20T00:00:00Z".to_string());
        {
            let mut watch = active.watch_state.lock().unwrap();
            watch.push_output(b"old terminal output");
            watch.push_transcript(wardian_core::control::WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "old chat answer".to_string(),
                provider: "codex".to_string(),
                turn_id: Some("turn-before-clear".to_string()),
                source: Some("transcript".to_string()),
            });
        }

        let prepared = prepare_agent_for_clear(&mut active);

        assert_eq!(prepared.termination.process_id, Some(12345));
        assert_eq!(
            prepared.config.session_id,
            active.config.lock().unwrap().session_id
        );
        assert_eq!(
            prepared.init_timestamp.as_deref(),
            Some("2026-05-20T00:00:00Z")
        );
        assert_eq!(active.process_id, None);
        assert!(active.stdin_tx.is_none());
        assert!(
            !Arc::ptr_eq(&runtime_status, &active.current_status),
            "clear must detach stale runtime status writers before replacement spawn"
        );
        assert_eq!(active.output_buffer.lock().unwrap().as_str(), "");
        assert_eq!(active.terminal_title.lock().unwrap().as_str(), "");
        assert_eq!(
            active.current_status.lock().unwrap().as_str(),
            "Processing..."
        );
        assert_eq!(*active.query_count.lock().unwrap(), 0);
        assert!(active.log_path.lock().unwrap().is_none());
        assert!(active.log_last_modified.lock().unwrap().is_none());
        let watch_snapshot = active
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .expect("watch snapshot after clear");
        assert!(watch_snapshot.output.text.is_empty());
        assert!(watch_snapshot.transcript.messages.is_empty());
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
    fn terminal_cleared_payload_uses_frontend_terminal_reset_contract() {
        assert_eq!(
            terminal_cleared_payload("agent-1"),
            serde_json::json!({ "session_id": "agent-1" })
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
    fn gemini_persists_resume_session_after_initial_spawn() {
        assert_eq!(
            persisted_resume_session_for_provider("gemini", None, "gemini-session-1"),
            Some("gemini-session-1".to_string())
        );
    }

    #[test]
    fn non_manual_session_providers_leave_resume_session_unchanged() {
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
    fn gemini_resume_without_recorded_resume_session_uses_manual_session_id() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session.as_deref(), Some("gemini-session"));
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
    fn antigravity_resume_without_conversation_id_does_not_use_wardian_uuid() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "antigravity".to_string(),
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
    fn codex_resume_clears_provider_thread_without_local_rollout_file() {
        let (_guard, temp) = use_isolated_resume_setting();
        let session_id = "22ff532b-007a-44c9-a4b4-9b7c0f546274";
        let codex_home = temp
            .path()
            .join("agents")
            .join(session_id)
            .join("habitat")
            .join(".codex");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: session_id.to_string(),
            resume_session: Some("019e15ff-5793-7bf3-b2fe-3be0233e26b1".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn codex_resume_does_not_adopt_index_thread_without_local_rollout_file() {
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
            codex_home.join("session_index.jsonl"),
            "{\"id\":\"019e15ff-5793-7bf3-b2fe-3be0233e26b1\",\"thread_name\":\"Stale\",\"updated_at\":\"2026-05-11T03:45:16.000Z\"}\n",
        )
        .expect("write index");
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: session_id.to_string(),
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
        let session_dir = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("20");
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        std::fs::write(
            session_dir
                .join("rollout-2026-04-20T00-00-00-019db2f3-22de-7861-8bc6-1b86db1686db.jsonl"),
            "",
        )
        .expect("write rollout file");
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
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                cleared_provider_sessions: vec![old_provider_session_id.to_string()],
                ..Default::default()
            }),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.codex_config().cleared_provider_sessions,
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
        let session_dir = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("20");
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        std::fs::write(
            session_dir.join(format!(
                "rollout-2026-04-20T00-00-00-{new_provider_session_id}.jsonl"
            )),
            "",
        )
        .expect("write rollout file");
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            session_id: session_id.to_string(),
            resume_session: None,
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                cleared_provider_sessions: vec![old_provider_session_id.to_string()],
                ..Default::default()
            }),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(
            config.resume_session.as_deref(),
            Some(new_provider_session_id)
        );
        assert!(config.codex_config().cleared_provider_sessions.is_empty());
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
    fn gemini_pause_resume_config_builds_resume_spawn_args() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session.as_deref(), Some("gemini-session"));
        let args = GeminiProvider::new().get_spawn_args(&config, true);
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"gemini-session".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn restored_gemini_startup_config_uses_resume_spawn_args() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: None,
            is_off: false,
            ..Default::default()
        };

        prepare_restored_config_for_spawn(&mut config).expect("prepare restored config");

        assert_eq!(config.resume_session.as_deref(), Some("gemini-session"));
        let args = GeminiProvider::new().get_spawn_args(&config, true);
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"gemini-session".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn restored_off_gemini_startup_config_stays_off() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: None,
            is_off: true,
            ..Default::default()
        };

        prepare_restored_config_for_spawn(&mut config).expect("prepare restored config");

        assert!(config.is_off);
        assert_eq!(config.resume_session, None);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn gemini_fresh_resume_uses_new_provider_session_without_changing_wardian_id() {
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
            session_id: "wardian-agent-id".to_string(),
            resume_session: Some("old-gemini-session".to_string()),
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
    fn gemini_empty_runtime_resume_starts_fresh_session() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "wardian-agent-id".to_string(),
            resume_session: Some("gemini-provider-session".to_string()),
            is_off: false,
            ..Default::default()
        };

        prepare_resume_config_for_runtime(&mut config, 0).expect("prepare resume config");

        assert_eq!(config.session_id, "wardian-agent-id");
        assert_eq!(config.resume_session, None);
        assert_ne!(
            config.fresh_provider_session_id.as_deref(),
            Some("wardian-agent-id")
        );
        assert_ne!(
            config.fresh_provider_session_id.as_deref(),
            Some("gemini-provider-session")
        );
        assert!(config.fresh_provider_session_id.is_some());
        let args = GeminiProvider::new().get_spawn_args(&config, false);
        assert!(args.contains(&"--session-id".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn off_gemini_runtime_resume_with_unknown_query_count_keeps_resume_session() {
        let (_guard, _temp) = use_isolated_resume_setting();
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "wardian-agent-id".to_string(),
            resume_session: Some("gemini-provider-session".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config_for_runtime(&mut config, 0).expect("prepare resume config");

        assert_eq!(
            config.resume_session.as_deref(),
            Some("gemini-provider-session")
        );
        assert_eq!(config.fresh_provider_session_id, None);
        assert!(!config.is_off);
        let args = GeminiProvider::new().get_spawn_args(&config, true);
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"gemini-provider-session".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
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

    #[test]
    fn prepare_clear_config_preserves_non_codex_provider_config() {
        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            provider_config: ProviderConfig::Gemini(GeminiProviderConfig {
                sandbox: Some(true),
                ..Default::default()
            }),
            resume_session: Some("old-gemini-session".to_string()),
            is_off: true,
            ..Default::default()
        };

        prepare_clear_config(&mut config).expect("prepare clear config");

        assert_eq!(config.provider, "gemini");
        assert_eq!(config.gemini_config().sandbox, Some(true));
        assert!(matches!(config.provider_config, ProviderConfig::Gemini(_)));
        assert_eq!(config.resume_session, None);
        assert!(config.fresh_provider_session_id.is_some());
        assert!(!config.is_off);
    }
}
