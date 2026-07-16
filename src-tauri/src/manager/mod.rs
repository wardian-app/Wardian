pub(crate) mod classes;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod headless;
pub(crate) mod opencode;
pub(crate) mod spawn;
#[cfg(test)]
mod spawn_tests;
pub(crate) mod telemetry;

// ── Re-exports for backward compatibility ───────────────────────────
// All external callers (lib.rs, commands/*) continue to use
// crate::manager::* exactly as before.

pub use classes::{
    get_agent_class_default_instruction, get_all_agent_classes, init_agent_classes, save_classes,
};
pub(crate) use codex::codex_session_exists_in_agent_home;
pub use codex::latest_codex_session_index_entry;
pub use headless::{
    obtain_session_id, run_headless, run_headless_with_config, run_headless_with_options,
    HeadlessRunOptions,
};
pub(crate) use opencode::opencode_last_assistant_text;
pub use opencode::{
    opencode_extract_created_session_id, opencode_extract_created_session_id_for_agent,
};
pub(crate) use opencode::{opencode_log_dirs, opencode_log_path_in};
pub use spawn::{resize_pty, spawn_agent};
pub use telemetry::{get_all_metrics, get_app_metrics};

pub use crate::utils::fs::*;
pub use crate::utils::logging::{log_debug, log_terminal_trace_bytes, log_terminal_trace_note};
pub use crate::utils::process::new_headless_command;
#[cfg(windows)]
pub use crate::utils::process::{
    app_process_supervisor_active, assign_pid_to_job, create_kill_on_close_job,
    find_wardian_session_process_roots, force_kill_process_tree,
};
pub use crate::utils::shell::build_program_launch;

use crate::state::{ActiveAgent, AppState};
use portable_pty::CommandBuilder;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::conversations::write_json_atomic;
use wardian_core::control::{ProviderInputReadiness, ProviderReadyEvidence};
use wardian_core::models::{AgentConfig, AgentEvent};
pub(crate) fn session_bootstrap_prompt() -> &'static str {
    "Introduce yourself"
}

#[cfg(windows)]
pub(crate) fn cleanup_stale_session_processes(session_id: &str, provider: &str) {
    let mut attempted = std::collections::BTreeSet::new();
    for _ in 0..2 {
        let roots = find_wardian_session_process_roots(session_id, Some(std::process::id()));
        let pending = roots
            .into_iter()
            .filter(|pid| attempted.insert(*pid))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }

        for pid in pending {
            log_debug(&format!(
                "[Wardian] Cleaning stale {} process tree for session {} via PID {}",
                provider, session_id, pid
            ));
            if let Err(err) = force_kill_process_tree(pid) {
                log_debug(&format!(
                    "[Wardian] Failed to clean stale process tree for session {} via PID {}: {}",
                    session_id, pid, err
                ));
            }
        }
    }
}

#[cfg(windows)]
pub(crate) fn cleanup_stale_persisted_session_processes() {
    let Some(app_dir) = get_wardian_home() else {
        return;
    };
    let state_path = app_dir.join("settings/state.json");
    let Ok(data) = std::fs::read_to_string(state_path) else {
        return;
    };
    let Ok(configs) = serde_json::from_str::<Vec<AgentConfig>>(&data) else {
        return;
    };

    let db_status_map = wardian_core::db::get_all_agents()
        .unwrap_or_default()
        .into_iter()
        .map(|agent| (agent.session_id, agent.last_status))
        .collect::<std::collections::HashMap<_, _>>();

    let sessions: Vec<(String, String)> = configs
        .into_iter()
        .filter(|config| {
            !config.is_off
                && db_status_map
                    .get(&config.session_id)
                    .and_then(|status| status.as_deref())
                    != Some("Headless")
        })
        .map(|config| (config.session_id, config.provider))
        .collect();
    if sessions.is_empty() {
        return;
    }

    // One system scan covers all sessions; scanning per session reads every
    // process's environment block once per agent and dominates startup time.
    let session_ids: Vec<String> = sessions.iter().map(|(id, _)| id.clone()).collect();
    let mut attempted = std::collections::BTreeSet::new();
    for _ in 0..2 {
        let roots_by_session =
            crate::utils::process::find_wardian_session_process_roots_for_sessions(
                &session_ids,
                Some(std::process::id()),
            );
        let mut killed_any = false;
        for (session_id, provider) in &sessions {
            for pid in roots_by_session
                .get(session_id)
                .into_iter()
                .flatten()
                .copied()
                .filter(|pid| attempted.insert(*pid))
            {
                killed_any = true;
                log_debug(&format!(
                    "[Wardian] Cleaning stale {} process tree for session {} via PID {}",
                    provider, session_id, pid
                ));
                if let Err(err) = force_kill_process_tree(pid) {
                    log_debug(&format!(
                        "[Wardian] Failed to clean stale process tree for session {} via PID {}: {}",
                        session_id, pid, err
                    ));
                }
            }
        }
        if !killed_any {
            break;
        }
    }
}

pub fn terminate_active_agent_process(agent: &mut ActiveAgent) {
    // IMPORTANT: Kill the process tree FIRST while the parent is still alive.
    // If we kill the PTY child (cmd.exe) first, its children (claude.exe, node.exe,
    // etc.) become orphaned and taskkill /T can no longer enumerate them via parent PID.
    #[cfg(windows)]
    {
        if let Some(pid) = agent.process_id {
            if let Err(err) = force_kill_process_tree(pid) {
                let sid = agent.config.lock().unwrap().session_id.clone();
                log_debug(&format!(
                    "[Wardian] Failed to force-kill process tree for session {} via PID {}: {}",
                    sid, pid, err
                ));
            }
        }
    }

    // Now kill the direct PTY child (may already be dead from the tree kill above).
    if let Some(mut child) = agent.child_process.take() {
        let _ = child.kill();
    }

    for mut child in agent.background_processes.drain(..) {
        #[cfg(windows)]
        {
            if let Err(err) = force_kill_process_tree(child.id()) {
                let sid = agent.config.lock().unwrap().session_id.clone();
                log_debug(&format!(
                    "[Wardian] Failed to force-kill background process for session {} via PID {}: {}",
                    sid,
                    child.id(),
                    err
                ));
            }
        }
        let _ = child.kill();
    }

    // Drop the Job Object last as a final safety net — its KILL_ON_JOB_CLOSE flag
    // will terminate any remaining processes still assigned to the job.
    #[cfg(windows)]
    {
        let _ = agent.job_object.take();
    }

    agent.process_id = None;
}

pub(crate) fn set_agent_status(
    app: &AppHandle,
    session_id: &str,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
    next_status: &str,
) {
    if let Ok(mut status) = current_status.lock() {
        if *status != next_status {
            *status = next_status.to_string();
            let observed_at =
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

            let status_app = app.clone();
            let status_session_id = session_id.to_string();
            let status = next_status.to_string();
            let current_status = current_status.clone();
            let status_sequence = app
                .state::<AppState>()
                .next_status_observation_sequence(session_id);
            tauri::async_runtime::spawn(async move {
                let state = status_app.state::<AppState>();
                if !status_arc_belongs_to_current_agent(&state, &status_session_id, &current_status)
                    .await
                {
                    log_debug(&format!(
                        "[Wardian] Ignoring stale status '{}' for replaced session {}",
                        status, status_session_id
                    ));
                    return;
                }

                // Phase 2: Persist status change to SQLite after confirming the
                // reporting runtime still owns the active agent slot.
                let _ = wardian_core::db::update_agent_status(&status_session_id, &status, None);

                record_provider_input_from_status_state(
                    &state,
                    &status_session_id,
                    &status,
                    status_sequence,
                )
                .await;
                let agents = state.agents.lock().await;
                if let Some(agent) = agents.get(&status_session_id) {
                    if let Ok(mut last_status_at) = agent.last_status_at.lock() {
                        *last_status_at = Some(observed_at.clone());
                    }
                    if let Ok(mut watch_state) = agent.watch_state.lock() {
                        watch_state.push_event(
                            "status",
                            serde_json::json!({
                                "status": wardian_core::identity::normalize_status(&status),
                                "observed_at": observed_at,
                            }),
                        );
                    }
                }
                drop(agents);

                let normalized_status = wardian_core::identity::normalize_status(&status);
                if matches!(normalized_status.as_str(), "idle" | "action_required") {
                    let archive_app = status_app.clone();
                    let archive_session_id = status_session_id.clone();
                    tauri::async_runtime::spawn(async move {
                        let state = archive_app.state::<AppState>();
                        if let Err(error) =
                            crate::commands::chat::archive_agent_chat_events_for_state(
                                state.inner(),
                                &archive_session_id,
                            )
                            .await
                        {
                            log_debug(&format!(
                                "[WARDIAN] conversation archive status sync failed for {archive_session_id}: {error}"
                            ));
                        }
                    });
                }

                let _ = status_app.emit(
                    "agent-status-updated",
                    serde_json::json!({
                        "session_id": status_session_id.clone(),
                        "current_status": status.clone(),
                    }),
                );
                crate::control::spawn_mailbox_drain_if_idle(
                    &status_app,
                    &status_session_id,
                    &status,
                );
            });
        }
    }
}

async fn status_arc_belongs_to_current_agent(
    state: &AppState,
    session_id: &str,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) -> bool {
    let agents = state.agents.lock().await;
    agents
        .get(session_id)
        .is_some_and(|agent| std::sync::Arc::ptr_eq(&agent.current_status, current_status))
}

async fn record_provider_input_from_status_state(
    state: &AppState,
    session_id: &str,
    next_status: &str,
    status_sequence: u64,
) {
    let readiness = match wardian_core::identity::normalize_status(next_status).as_str() {
        "idle" => ProviderInputReadiness::Ready,
        "processing" => ProviderInputReadiness::Busy,
        "action_required" => ProviderInputReadiness::ActionRequired,
        "off" | "error" => ProviderInputReadiness::Unavailable,
        _ => ProviderInputReadiness::Unknown,
    };
    let evidence = (readiness == ProviderInputReadiness::Ready)
        .then_some(ProviderReadyEvidence::ProviderEvent);
    let generation = state
        .interactions
        .current_provider_input_generation(session_id)
        .await
        .unwrap_or(0);
    state
        .interactions
        .record_provider_input_status_observation(
            session_id,
            status_sequence,
            generation,
            readiness,
            evidence,
        )
        .await;
}

pub(crate) fn emit_agent_turn_completed(app: &AppHandle, session_id: &str) {
    let _ = app.emit(
        "agent-turn-completed",
        serde_json::json!({
            "session_id": session_id,
        }),
    );
}

pub(crate) fn mark_agent_prompt_started(agent: &crate::state::ActiveAgent) -> bool {
    let current_status = agent
        .current_status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default();
    if current_status == "Action Needed" || current_status == "Off" {
        return false;
    }

    if let Ok(mut count) = agent.query_count.lock() {
        *count += 1;
    }

    current_status != "Processing..."
}

pub(crate) fn debug_preview_bytes(bytes: &[u8], limit: usize) -> String {
    let mut out = String::new();
    for &byte in bytes.iter().take(limit) {
        match byte {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x1b => out.push_str("\\x1b"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\x{:02x}", byte)),
        }
    }
    if bytes.len() > limit {
        out.push_str("...");
    }
    out
}

pub(crate) fn extract_terminal_titles(chunk: &str) -> Vec<String> {
    let bytes = chunk.as_bytes();
    let mut titles = Vec::new();
    let mut index = 0usize;

    while index + 2 < bytes.len() {
        if bytes[index] == 0x1b && bytes[index + 1] == b']' {
            let mut cursor = index + 2;
            while cursor < bytes.len() && bytes[cursor] != b';' {
                cursor += 1;
            }
            if cursor >= bytes.len() {
                break;
            }

            let code = String::from_utf8_lossy(&bytes[index + 2..cursor]);
            if code != "0" && code != "2" {
                index = cursor.saturating_add(1);
                continue;
            }

            let value_start = cursor + 1;
            let mut end = value_start;
            while end < bytes.len() {
                if bytes[end] == 0x07 {
                    break;
                }
                if bytes[end] == 0x1b && end + 1 < bytes.len() && bytes[end + 1] == b'\\' {
                    break;
                }
                end += 1;
            }

            let title = String::from_utf8_lossy(&bytes[value_start..end])
                .trim()
                .to_string();
            if !title.is_empty() {
                titles.push(title);
            }

            index = end.saturating_add(1);
            continue;
        }

        index += 1;
    }

    titles
}

pub(crate) fn apply_agent_event(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    query_count: &std::sync::Arc<std::sync::Mutex<usize>>,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) {
    apply_agent_event_with_policy(
        app,
        session_id,
        event,
        query_count,
        init_timestamp,
        current_status,
        ProviderStatusEventPolicy::Normal,
    );
}

pub(crate) fn apply_agent_event_with_policy(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    query_count: &std::sync::Arc<std::sync::Mutex<usize>>,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
    policy: ProviderStatusEventPolicy,
) {
    match &event {
        AgentEvent::UserQuery => {
            if let Ok(mut count) = query_count.lock() {
                *count += 1;
            }
        }
        AgentEvent::Init { timestamp, .. } => {
            if let Ok(mut ts) = init_timestamp.lock() {
                *ts = timestamp.clone();
            }
        }
        _ => {}
    }
    apply_agent_status_event_with_policy(app, session_id, event, current_status, policy);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderStatusEventPolicy {
    Normal,
    PreserveActionRequired,
}

pub(crate) fn provider_status_from_event(
    current_status: &str,
    event: &AgentEvent,
    policy: ProviderStatusEventPolicy,
) -> Option<&'static str> {
    match event {
        AgentEvent::UserQuery | AgentEvent::Generating => {
            if policy == ProviderStatusEventPolicy::PreserveActionRequired
                && wardian_core::identity::normalize_status(current_status) == "action_required"
            {
                None
            } else {
                Some("Processing...")
            }
        }
        AgentEvent::ModelResponse | AgentEvent::TurnCompleted => Some("Idle"),
        AgentEvent::ActionRequired { .. } => Some("Action Needed"),
        AgentEvent::Init { .. } | AgentEvent::Unknown => None,
    }
}

pub(crate) fn apply_agent_status_event_with_policy(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
    policy: ProviderStatusEventPolicy,
) {
    let current = current_status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default();
    if let Some(next_status) = provider_status_from_event(&current, &event, policy) {
        set_agent_status(app, session_id, current_status, next_status);
        if matches!(event, AgentEvent::TurnCompleted) {
            emit_agent_turn_completed(app, session_id);
        }
    }
}

pub(crate) fn apply_agent_status_event(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) {
    apply_agent_status_event_with_policy(
        app,
        session_id,
        event,
        current_status,
        ProviderStatusEventPolicy::Normal,
    );
}

/// On macOS, GUI apps inherit a minimal PATH that excludes Homebrew, npm globals,
/// Volta, and other user-level tool installs. Prepend the common locations so that
/// `claude`, `gemini`, and similar CLIs can be found when spawning child processes.
#[cfg(target_os = "macos")]
pub(crate) fn macos_extended_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let existing = std::env::var("PATH").unwrap_or_default();
    let extra = format!(
        "{home}/.local/bin:/usr/local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:{home}/.npm-global/bin:{home}/.volta/bin",
        home = home
    );
    let extended = if existing.is_empty() {
        format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", extra)
    } else {
        format!("{}:{}", extra, existing)
    };
    crate::utils::cli_install::child_path_with_cli_bin(Some(&extended)).unwrap_or(extended)
}

pub(crate) fn state_configs_snapshot(
    agents: &HashMap<String, ActiveAgent>,
    order: &[String],
) -> Vec<AgentConfig> {
    let mut configs = Vec::new();
    for id in order {
        if let Some(agent) = agents.get(id) {
            configs.push(agent.config.lock().unwrap().clone());
        }
    }
    configs
}

pub(crate) fn try_save_state_snapshot(configs: &[AgentConfig]) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or_else(|| "Could not locate Wardian home".to_string())?;
    std::fs::create_dir_all(&app_dir).map_err(|error| error.to_string())?;
    let settings_dir = app_dir.join("settings");
    std::fs::create_dir_all(&settings_dir).map_err(|error| error.to_string())?;
    write_json_atomic(&settings_dir.join("state.json"), configs)
        .map_err(|error| error.to_string())
}

pub(crate) fn save_state_snapshot(_app: &AppHandle, configs: &[AgentConfig]) {
    if let Err(error) = try_save_state_snapshot(configs) {
        log_debug(&format!("[WARDIAN] Failed to persist state snapshot: {error}"));
    }
}

pub fn save_state(app: &AppHandle, agents: &HashMap<String, ActiveAgent>, order: &[String]) {
    let configs = state_configs_snapshot(agents, order);
    save_state_snapshot(app, &configs);
}

pub(crate) fn strip_flag_value_pairs(args: Vec<String>, flag: &str) -> Vec<String> {
    let mut stripped = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            let _ = iter.next();
            continue;
        }
        stripped.push(arg);
    }
    stripped
}

pub(crate) fn strip_flag_value_pairs_and_equals(args: Vec<String>, flag: &str) -> Vec<String> {
    let equals_prefix = format!("{flag}=");
    let mut stripped = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            let _ = iter.next();
            continue;
        }
        if arg.starts_with(&equals_prefix) {
            continue;
        }
        stripped.push(arg);
    }
    stripped
}

pub(crate) fn strip_standalone_flag(args: Vec<String>, flag: &str) -> Vec<String> {
    args.into_iter().filter(|arg| arg != flag).collect()
}

pub(crate) fn persisted_agent_config(session_id: &str) -> Option<AgentConfig> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }

    let wardian_home = get_wardian_home()?;
    let state_path = wardian_home.join("settings/state.json");
    let contents = std::fs::read_to_string(state_path).ok()?;
    let configs = serde_json::from_str::<Vec<AgentConfig>>(&contents).ok()?;
    configs
        .into_iter()
        .find(|config| config.session_id == session_id)
}

pub(crate) fn interactive_provider_cwd(
    provider_name: &str,
    workspace_cwd: &std::path::Path,
    habitat_root: Option<&std::path::Path>,
    codex_bootstrap: Option<&(std::path::PathBuf, std::path::PathBuf)>,
) -> std::path::PathBuf {
    if let Some((provider_cwd, _)) = codex_bootstrap {
        return provider_cwd.clone();
    }

    if provider_name == "codex" {
        workspace_cwd.to_path_buf()
    } else if provider_name == "opencode" {
        habitat_root
            .map(|root| root.to_path_buf())
            .unwrap_or_else(|| workspace_cwd.to_path_buf())
    } else {
        habitat_root
            .map(habitat_workspace_cwd)
            .unwrap_or_else(|| workspace_cwd.to_path_buf())
    }
}

pub(crate) fn worktree_build_env(config: &AgentConfig) -> Vec<(String, String)> {
    if config.git_worktree != Some(true) {
        return Vec::new();
    }

    let Some(source_folder) = config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
    else {
        return Vec::new();
    };

    let source_path = std::path::Path::new(source_folder);
    if !source_path.join("Cargo.toml").is_file() {
        return Vec::new();
    }

    vec![(
        "CARGO_TARGET_DIR".to_string(),
        source_path.join("target").to_string_lossy().to_string(),
    )]
}

pub(crate) fn interactive_provider_args(
    provider_name: &str,
    provider_cwd: &std::path::Path,
    workspace_cwd: &std::path::Path,
    mut provider_args: Vec<String>,
) -> Vec<String> {
    match provider_name {
        "codex" => {
            provider_args.push("--cd".to_string());
            provider_args.push(provider_cwd.to_string_lossy().to_string());
        }
        "opencode" => {
            let target_dir = if provider_cwd
                .file_name()
                .is_some_and(|name| name == "habitat")
            {
                habitat_workspace_cwd(provider_cwd)
            } else {
                workspace_cwd.to_path_buf()
            };
            provider_args.push(target_dir.to_string_lossy().replace('\\', "/"));
        }
        "antigravity" => {
            provider_args.push("--prompt-interactive".to_string());
            provider_args.push(String::new());
        }
        _ => {}
    }

    provider_args
}

pub(crate) fn finalize_interactive_spawn_args(
    provider_name: &str,
    _is_restored: bool,
    _resume_session: &Option<String>,
    provider_args: Vec<String>,
) -> Vec<String> {
    if provider_name == "claude" {
        let provider_args = strip_standalone_flag(provider_args, "--verbose");
        let provider_args = strip_flag_value_pairs_and_equals(provider_args, "--input-format");
        return strip_flag_value_pairs_and_equals(provider_args, "--output-format");
    }

    provider_args
}

pub(crate) fn interactive_provider_launch(
    provider_name: &str,
    bin: &str,
    provider_args: &[String],
) -> Result<crate::utils::shell::ShellLaunchSpec, String> {
    #[cfg(windows)]
    if matches!(provider_name, "opencode" | "antigravity") {
        let bin_path = std::path::Path::new(bin);
        let is_native_exe = bin_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("exe"));
        if !is_native_exe {
            return build_program_launch(bin, provider_args);
        }
    }

    let _ = provider_name;
    Ok(crate::utils::shell::ShellLaunchSpec {
        executable: bin.to_string(),
        args: provider_args.to_vec(),
    })
}

pub(crate) fn apply_terminal_identity_env(cmd: &mut CommandBuilder) {
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM", "xterm-256color");
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        cmd.env("WARDIAN_HOME", home);
    }
    apply_managed_cli_path_to_pty(cmd);
}

pub(crate) fn apply_managed_cli_path_to_pty(cmd: &mut CommandBuilder) {
    if let Some(path) =
        crate::utils::cli_install::child_path_with_cli_bin(std::env::var("PATH").ok().as_deref())
    {
        cmd.env(managed_cli_path_env_key(), path);
    }
}

pub(crate) fn apply_managed_cli_path_to_process(cmd: &mut tokio::process::Command) {
    if let Some(path) =
        crate::utils::cli_install::child_path_with_cli_bin(std::env::var("PATH").ok().as_deref())
    {
        cmd.env(managed_cli_path_env_key(), path);
    }
}

#[cfg(target_os = "windows")]
fn managed_cli_path_env_key() -> &'static str {
    "Path"
}

#[cfg(not(target_os = "windows"))]
fn managed_cli_path_env_key() -> &'static str {
    "PATH"
}

pub(crate) fn apply_interactive_provider_runtime_env(
    provider_name: &str,
    cmd: &mut CommandBuilder,
) -> Result<(), String> {
    if provider_name == "claude" {
        for (key, value) in claude_terminal_runtime_env() {
            cmd.env(key, value);
        }
    }

    #[cfg(windows)]
    if provider_name == "claude" {
        if let Some(script) = ensure_claude_bash_env_script()? {
            cmd.env("BASH_ENV", script);
        }
    }

    let _ = (provider_name, cmd);

    Ok(())
}

pub(crate) fn apply_process_provider_runtime_env(
    provider_name: &str,
    cmd: &mut tokio::process::Command,
) -> Result<(), String> {
    if provider_name == "claude" {
        for (key, value) in claude_terminal_runtime_env() {
            cmd.env(key, value);
        }
    }

    #[cfg(windows)]
    if provider_name == "claude" {
        if let Some(script) = ensure_claude_bash_env_script()? {
            cmd.env("BASH_ENV", script);
        }
    }

    let _ = (provider_name, cmd);

    Ok(())
}

pub(crate) fn claude_terminal_runtime_env() -> [(&'static str, &'static str); 2] {
    [
        ("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1"),
        ("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1"),
    ]
}

#[cfg(windows)]
fn ensure_claude_bash_env_script() -> Result<Option<String>, String> {
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        return Ok(None);
    };
    let script_path = home
        .join("runtime")
        .join("windows")
        .join("claude-bash-env.sh");
    if let Some(parent) = script_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create Claude bash environment directory {}: {err}",
                parent.display()
            )
        })?;
    }
    let bin_path = windows_path_to_msys_shell_path(&home.join("bin"));
    let contents = format!(
        "# wardian Claude tool shell PATH\nwardian_bin={}\ncase \":$PATH:\" in\n  *\":$wardian_bin:\"*) ;;\n  *) export PATH=\"$wardian_bin:$PATH\" ;;\nesac\n",
        shell_single_quote(&bin_path)
    );
    std::fs::write(&script_path, contents).map_err(|err| {
        format!(
            "Failed to write Claude bash environment script {}: {err}",
            script_path.display()
        )
    })?;
    Ok(Some(windows_path_to_msys_shell_path(&script_path)))
}

#[cfg(windows)]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn windows_path_to_msys_shell_path(path: &std::path::Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        format!("/{drive}{}", &text[2..])
    } else {
        text
    }
}

pub(crate) fn display_log_path(path: &std::path::Path) -> String {
    let display_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let display = display_path.to_string_lossy();
    #[cfg(windows)]
    {
        display
            .strip_prefix(r"\\?\")
            .unwrap_or(&display)
            .to_string()
    }
    #[cfg(not(windows))]
    {
        display.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn try_save_state_snapshot_reports_write_failures() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let blocked_home = temp.path().join("blocked-home");
        std::fs::write(&blocked_home, "not a directory").expect("create blocking file");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", &blocked_home) };

        let result = try_save_state_snapshot(&[AgentConfig::default()]);

        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }
        assert!(result.is_err());
    }

    #[test]
    fn try_save_state_snapshot_replaces_existing_state_atomically() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("wardian-home");
        std::fs::create_dir_all(home.join("settings")).expect("create settings dir");
        std::fs::write(home.join("settings/state.json"), r#"[{"session_id":"old"}]"#)
            .expect("seed old snapshot");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", &home) };

        try_save_state_snapshot(&[AgentConfig::default()]).expect("atomic snapshot write");

        let contents =
            std::fs::read_to_string(home.join("settings/state.json")).expect("read snapshot");
        let configs: Vec<AgentConfig> = serde_json::from_str(&contents).expect("parse snapshot");
        assert_eq!(configs.len(), 1);
        assert!(!home.join("settings/.state.json.tmp").exists());

        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }
    }

    #[test]
    fn antigravity_interactive_launch_supplies_empty_prompt_value() {
        let args = interactive_provider_args(
            "antigravity",
            Path::new("/workspace"),
            Path::new("/workspace"),
            Vec::new(),
        );

        assert_eq!(
            args,
            vec!["--prompt-interactive".to_string(), String::new()]
        );
    }

    #[cfg(windows)]
    #[test]
    fn managed_cli_path_is_applied_to_pty_commands_with_windows_path_key() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let previous_path = std::env::var_os("PATH");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        std::env::set_var("PATH", r"C:\Windows\System32");

        let mut cmd = CommandBuilder::new("claude");
        apply_managed_cli_path_to_pty(&mut cmd);

        let path_keys = cmd
            .iter_extra_env_as_str()
            .filter_map(|(key, _)| key.eq_ignore_ascii_case("PATH").then_some(key.to_string()))
            .collect::<Vec<_>>();
        assert_eq!(path_keys, vec!["Path".to_string()]);
        let path = cmd
            .get_env("Path")
            .expect("Path env")
            .to_string_lossy()
            .to_string();
        assert!(path.starts_with(&home.path().join("bin").display().to_string()));
        assert!(path.ends_with(r"C:\Windows\System32"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn antigravity_interactive_launch_uses_configured_shell_for_windows_cmd_shims() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let previous_comspec = std::env::var_os("ComSpec");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        std::env::set_var(
            "ComSpec",
            r"D:\Development\Wardian\target\release\Wardian.exe",
        );
        let settings_path = home.path().join("settings").join("shell.json");
        std::fs::create_dir_all(settings_path.parent().expect("settings parent")).unwrap();
        std::fs::write(
            &settings_path,
            r#"{
              "shell_id": "custom",
              "custom_executable": "pwsh.exe",
              "custom_args": "-NoProfile -Command",
              "agent_session_persistence": "resume"
            }"#,
        )
        .unwrap();

        let launch = interactive_provider_launch(
            "antigravity",
            r"C:\Users\test\AppData\Roaming\npm\agy.cmd",
            &["--prompt-interactive".to_string(), String::new()],
        )
        .expect("launch");

        assert_eq!(launch.executable, "pwsh.exe");
        assert_eq!(launch.args[0], "-NoProfile");
        assert_eq!(launch.args[1], "-Command");
        assert!(launch.args[2].contains("agy.cmd"));
        assert!(launch.args[2].contains("--prompt-interactive"));
        assert!(!launch.args[2].contains("ComSpec"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_comspec {
            Some(value) => std::env::set_var("ComSpec", value),
            None => std::env::remove_var("ComSpec"),
        }
    }

    #[test]
    fn terminal_identity_env_includes_resolved_wardian_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());

        let mut cmd = CommandBuilder::new("claude");
        apply_terminal_identity_env(&mut cmd);

        let wardian_home = cmd
            .get_env("WARDIAN_HOME")
            .expect("WARDIAN_HOME env")
            .to_string_lossy()
            .to_string();
        assert_eq!(wardian_home, home.path().display().to_string());

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn terminal_identity_env_includes_managed_cli_path() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let previous_path = std::env::var_os("PATH");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        let existing_path = if cfg!(windows) {
            r"C:\Windows\System32"
        } else {
            "/usr/bin"
        };
        std::env::set_var("PATH", existing_path);

        let mut cmd = CommandBuilder::new("pwsh");
        apply_terminal_identity_env(&mut cmd);

        let path = cmd
            .get_env(managed_cli_path_env_key())
            .expect("managed CLI path env")
            .to_string_lossy()
            .to_string();
        assert!(path.starts_with(&home.path().join("bin").display().to_string()));
        assert!(path.ends_with(existing_path));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn provider_runtime_env_does_not_inject_shell_or_node_hooks() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_comspec = std::env::var_os("ComSpec");
        let previous_node_options = std::env::var_os("NODE_OPTIONS");
        let previous_shim = std::env::var_os("WARDIAN_SILENT_CMD_SHIM");
        let previous_real_comspec = std::env::var_os("WARDIAN_REAL_COMSPEC");

        std::env::set_var("ComSpec", r"C:\Windows\System32\cmd.exe");
        std::env::remove_var("NODE_OPTIONS");
        std::env::remove_var("WARDIAN_SILENT_CMD_SHIM");
        std::env::remove_var("WARDIAN_REAL_COMSPEC");

        let mut interactive = CommandBuilder::new("gemini");
        apply_interactive_provider_runtime_env("gemini", &mut interactive).unwrap();
        let interactive_env = interactive
            .iter_extra_env_as_str()
            .map(|(key, _)| key.to_string())
            .collect::<Vec<_>>();
        assert!(!interactive_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("ComSpec")));
        assert!(!interactive_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("NODE_OPTIONS")));
        assert!(!interactive_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("WARDIAN_SILENT_CMD_SHIM")));
        assert!(!interactive_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("WARDIAN_REAL_COMSPEC")));

        let mut process = tokio::process::Command::new("antigravity");
        apply_process_provider_runtime_env("antigravity", &mut process).unwrap();
        let process_env = process
            .as_std()
            .get_envs()
            .map(|(key, _)| key.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(!process_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("ComSpec")));
        assert!(!process_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("NODE_OPTIONS")));
        assert!(!process_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("WARDIAN_SILENT_CMD_SHIM")));
        assert!(!process_env
            .iter()
            .any(|key| key.eq_ignore_ascii_case("WARDIAN_REAL_COMSPEC")));

        match previous_comspec {
            Some(value) => std::env::set_var("ComSpec", value),
            None => std::env::remove_var("ComSpec"),
        }
        match previous_node_options {
            Some(value) => std::env::set_var("NODE_OPTIONS", value),
            None => std::env::remove_var("NODE_OPTIONS"),
        }
        match previous_shim {
            Some(value) => std::env::set_var("WARDIAN_SILENT_CMD_SHIM", value),
            None => std::env::remove_var("WARDIAN_SILENT_CMD_SHIM"),
        }
        match previous_real_comspec {
            Some(value) => std::env::set_var("WARDIAN_REAL_COMSPEC", value),
            None => std::env::remove_var("WARDIAN_REAL_COMSPEC"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn claude_provider_runtime_env_sets_bash_env_path_hook() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());

        let mut interactive = CommandBuilder::new("claude");
        apply_interactive_provider_runtime_env("claude", &mut interactive).unwrap();

        let bash_env = interactive
            .get_env("BASH_ENV")
            .expect("BASH_ENV env")
            .to_string_lossy()
            .to_string();
        assert!(bash_env.ends_with("/runtime/windows/claude-bash-env.sh"));
        let script_path = home
            .path()
            .join("runtime")
            .join("windows")
            .join("claude-bash-env.sh");
        let script = std::fs::read_to_string(&script_path).expect("bash env script");
        assert!(script.contains("export PATH=\"$wardian_bin:$PATH\""));
        assert!(script.contains(&windows_path_to_msys_shell_path(&home.path().join("bin"))));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn claude_interactive_runtime_env_disables_alternate_screen() {
        let mut interactive = CommandBuilder::new("claude");
        apply_interactive_provider_runtime_env("claude", &mut interactive).unwrap();

        let disable_alt_screen = interactive
            .get_env("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN")
            .expect("Claude alternate-screen opt-out env")
            .to_string_lossy()
            .to_string();
        assert_eq!(disable_alt_screen, "1");
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_to_msys_shell_path_converts_drive_paths() {
        assert_eq!(
            windows_path_to_msys_shell_path(Path::new(r"D:\Development\Wardian\.wardian\bin")),
            "/d/Development/Wardian/.wardian/bin"
        );
    }

    #[cfg(windows)]
    #[test]
    fn managed_cli_path_is_applied_to_headless_processes() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let previous_path = std::env::var_os("PATH");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        std::env::set_var("PATH", r"C:\Windows\System32");

        let mut cmd = tokio::process::Command::new("claude");
        apply_managed_cli_path_to_process(&mut cmd);

        let path = cmd
            .as_std()
            .get_envs()
            .find_map(|(key, value)| {
                (key == managed_cli_path_env_key())
                    .then(|| value.expect("PATH value").to_string_lossy().to_string())
            })
            .expect("managed CLI path env");
        assert!(path.starts_with(&home.path().join("bin").display().to_string()));
        assert!(path.ends_with(r"C:\Windows\System32"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }

    #[test]
    fn extract_terminal_titles_reads_bel_and_st_sequences() {
        let chunk = "\u{1b}]0;OpenCode\u{7}x\u{1b}]2;OC | Working\u{1b}\\";

        assert_eq!(
            extract_terminal_titles(chunk),
            vec!["OpenCode".to_string(), "OC | Working".to_string()]
        );
    }

    #[test]
    fn display_log_path_canonicalizes_existing_paths_for_ui() {
        let temp = tempfile::tempdir().expect("temp dir");
        let nested = temp.path().join("sessions");
        std::fs::create_dir_all(&nested).expect("sessions dir");
        let log = nested.join("session.jsonl");
        std::fs::write(&log, "{}\n").expect("log file");
        let indirect = nested.join("..").join("sessions").join("session.jsonl");

        let expected_path = log.canonicalize().unwrap();
        let expected_text = expected_path.to_string_lossy();
        let expected = expected_text
            .strip_prefix(r"\\?\")
            .unwrap_or(&expected_text);

        assert_eq!(display_log_path(&indirect), expected);
    }

    #[test]
    fn display_log_path_preserves_missing_paths() {
        let missing = Path::new("C:/path/that/does/not/exist/session.jsonl");

        assert_eq!(display_log_path(missing), missing.to_string_lossy());
    }

    #[test]
    fn strip_flag_value_pairs_removes_all_add_dir_arguments() {
        let args = vec![
            "--model".to_string(),
            "gpt-5".to_string(),
            "--add-dir".to_string(),
            "C:/Users/test/.wardian/common".to_string(),
            "--add-dir".to_string(),
            "C:/Users/test/.wardian/classes/Coder".to_string(),
            "--search".to_string(),
        ];

        let stripped = strip_flag_value_pairs(args, "--add-dir");

        assert_eq!(
            stripped,
            vec![
                "--model".to_string(),
                "gpt-5".to_string(),
                "--search".to_string(),
            ]
        );
    }

    #[test]
    fn claude_interactive_spawn_strips_equals_form_print_only_stream_flags() {
        let args = finalize_interactive_spawn_args(
            "claude",
            false,
            &None,
            vec![
                "--input-format=stream-json".to_string(),
                "--output-format=stream-json".to_string(),
                "--model".to_string(),
                "claude-test".to_string(),
                "--mcp-config=server.json".to_string(),
            ],
        );

        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "claude-test".to_string(),
                "--mcp-config=server.json".to_string(),
            ]
        );
    }

    #[test]
    fn strip_standalone_flag_removes_only_the_matching_flag() {
        let args = vec![
            "resume".to_string(),
            "session-abc".to_string(),
            "--no-alt-screen".to_string(),
            "--model".to_string(),
            "gpt-5.4".to_string(),
        ];

        let stripped = strip_standalone_flag(args, "--no-alt-screen");

        assert_eq!(
            stripped,
            vec![
                "resume".to_string(),
                "session-abc".to_string(),
                "--model".to_string(),
                "gpt-5.4".to_string(),
            ]
        );
    }

    #[test]
    fn opencode_interactive_launch_uses_habitat_root() {
        let workspace_cwd = Path::new("D:/Development/Wardian");
        let habitat_root = Some(Path::new("C:/Users/test/.wardian/agents/ses_test/habitat"));

        let provider_cwd = interactive_provider_cwd("opencode", workspace_cwd, habitat_root, None);

        // OpenCode starts from the habitat root so its project-local skill walk-up
        // sees habitat/.opencode and habitat/.agents before switching to the real
        // workspace via --dir.
        assert_eq!(
            provider_cwd,
            Path::new("C:/Users/test/.wardian/agents/ses_test/habitat")
        );
    }

    #[test]
    fn opencode_interactive_launch_matches_bootstrap_workspace() {
        let workspace_cwd = Path::new("D:/Development/Wardian");
        let habitat_root = Some(Path::new("C:/Users/test/.wardian/agents/ses_test/habitat"));

        let interactive_cwd =
            interactive_provider_cwd("opencode", workspace_cwd, habitat_root, None);

        assert_eq!(
            interactive_cwd,
            Path::new("C:/Users/test/.wardian/agents/ses_test/habitat")
        );
    }

    #[test]
    fn fresh_opencode_interactive_spawn_keeps_explicit_session_after_bootstrap() {
        let args = finalize_interactive_spawn_args(
            "opencode",
            false,
            &Some("ses_test".to_string()),
            vec!["--session".to_string(), "ses_test".to_string()],
        );

        assert_eq!(args, vec!["--session".to_string(), "ses_test".to_string()]);
    }

    #[test]
    fn restored_opencode_interactive_spawn_keeps_explicit_session() {
        let args = finalize_interactive_spawn_args(
            "opencode",
            true,
            &Some("ses_test".to_string()),
            vec!["--session".to_string(), "ses_test".to_string()],
        );

        assert_eq!(args, vec!["--session".to_string(), "ses_test".to_string()]);
    }

    #[test]
    fn codex_interactive_spawn_preserves_inline_scrollback_mode() {
        let args = finalize_interactive_spawn_args(
            "codex",
            true,
            &Some("019d331a-0500-7592-969f-8f437886f42b".to_string()),
            vec![
                "resume".to_string(),
                "019d331a-0500-7592-969f-8f437886f42b".to_string(),
                "--no-alt-screen".to_string(),
                "--model".to_string(),
                "gpt-5.4".to_string(),
            ],
        );

        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "019d331a-0500-7592-969f-8f437886f42b".to_string(),
                "--no-alt-screen".to_string(),
                "--model".to_string(),
                "gpt-5.4".to_string(),
            ]
        );
    }

    #[test]
    fn claude_interactive_spawn_strips_print_only_stream_json_flags() {
        let args = finalize_interactive_spawn_args(
            "claude",
            true,
            &Some("claude-session".to_string()),
            vec![
                "--verbose".to_string(),
                "--input-format".to_string(),
                "stream-json".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--model".to_string(),
                "claude-test".to_string(),
                "--resume".to_string(),
                "claude-session".to_string(),
                "--add-dir".to_string(),
                "C:/Users/test/.wardian/classes/Coder".to_string(),
            ],
        );

        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "claude-test".to_string(),
                "--resume".to_string(),
                "claude-session".to_string(),
                "--add-dir".to_string(),
                "C:/Users/test/.wardian/classes/Coder".to_string(),
            ]
        );
    }

    fn test_active_agent(status: &str) -> crate::state::ActiveAgent {
        crate::state::ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(AgentConfig::default())),
            child_process: None,
            background_processes: Vec::new(),
            runtime_generation: None,
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new(status.to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::AgentWatchState::new("test-agent".to_string(), 4096, 262_144),
            )),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    #[test]
    fn state_configs_snapshot_uses_order_without_writing_state() {
        let mut agents = HashMap::new();
        let first = test_active_agent("Idle");
        {
            let mut config = first.config.lock().unwrap();
            config.session_id = "agent-1".to_string();
            config.session_name = "First".to_string();
        }
        let second = test_active_agent("Idle");
        {
            let mut config = second.config.lock().unwrap();
            config.session_id = "agent-2".to_string();
            config.session_name = "Second".to_string();
        }
        agents.insert("agent-1".to_string(), first);
        agents.insert("agent-2".to_string(), second);

        let snapshot =
            state_configs_snapshot(&agents, &["agent-2".to_string(), "missing".to_string()]);

        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].session_id, "agent-2");
        assert_eq!(snapshot[0].session_name, "Second");
    }

    #[test]
    fn mark_agent_prompt_started_counts_query_and_requests_processing_transition() {
        let agent = test_active_agent("Idle");

        assert!(mark_agent_prompt_started(&agent));

        assert_eq!(agent.current_status.lock().unwrap().as_str(), "Idle");
        assert_eq!(*agent.query_count.lock().unwrap(), 1);
    }

    #[test]
    fn mark_agent_prompt_started_preserves_action_needed() {
        let agent = test_active_agent("Action Needed");

        assert!(!mark_agent_prompt_started(&agent));

        assert_eq!(
            agent.current_status.lock().unwrap().as_str(),
            "Action Needed"
        );
        assert_eq!(*agent.query_count.lock().unwrap(), 0);
    }

    #[test]
    fn status_event_policy_preserves_action_needed_until_explicit_completion() {
        assert_eq!(
            provider_status_from_event(
                "Action Needed",
                &AgentEvent::Generating,
                ProviderStatusEventPolicy::PreserveActionRequired,
            ),
            None
        );

        assert_eq!(
            provider_status_from_event(
                "Action Needed",
                &AgentEvent::ModelResponse,
                ProviderStatusEventPolicy::PreserveActionRequired,
            ),
            Some("Idle")
        );
    }

    #[tokio::test]
    async fn stale_runtime_status_arc_is_not_current_after_agent_replacement() {
        let state = AppState::new();
        let old_agent = test_active_agent("Idle");
        let old_status = old_agent.current_status.clone();
        {
            let mut config = old_agent.config.lock().unwrap();
            config.session_id = "agent-1".to_string();
        }
        state
            .agents
            .lock()
            .await
            .insert("agent-1".to_string(), old_agent);

        let new_agent = test_active_agent("Idle");
        {
            let mut config = new_agent.config.lock().unwrap();
            config.session_id = "agent-1".to_string();
        }
        state
            .agents
            .lock()
            .await
            .insert("agent-1".to_string(), new_agent);

        assert!(
            !status_arc_belongs_to_current_agent(&state, "agent-1", &old_status).await,
            "late status events from a cleared runtime must not update the replacement agent"
        );
    }

    #[tokio::test]
    async fn provider_status_events_update_input_readiness_before_drain() {
        let state = AppState::new();
        state
            .interactions
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Booting, None)
            .await;

        record_provider_input_from_status_state(&state, "agent-1", "Action Needed", 1).await;
        record_provider_input_from_status_state(&state, "agent-1", "Idle", 2).await;

        let current = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(current.state, ProviderInputReadiness::Ready);
        assert_eq!(current.generation, 1);
        assert_eq!(
            current.ready_evidence,
            Some(ProviderReadyEvidence::ProviderEvent)
        );
    }

    #[test]
    fn worktree_build_env_points_cargo_target_dir_to_source_checkout() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("Wardian");
        let worktree = temp.path().join("Wardian.wt").join("debugging");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::write(source.join("Cargo.toml"), "[workspace]\n").expect("cargo toml");
        let config = AgentConfig {
            git_worktree: Some(true),
            git_worktree_source: Some(source.to_string_lossy().to_string()),
            git_worktree_folder: Some(worktree.to_string_lossy().to_string()),
            ..Default::default()
        };

        let env = worktree_build_env(&config);

        assert!(env.contains(&(
            "CARGO_TARGET_DIR".to_string(),
            source.join("target").to_string_lossy().to_string(),
        )));
    }

    #[test]
    fn worktree_build_env_skips_non_worktree_agents() {
        let config = AgentConfig {
            folder: "D:/Development/Wardian".to_string(),
            ..Default::default()
        };

        assert!(worktree_build_env(&config).is_empty());
    }

    #[tokio::test]
    async fn older_provider_status_observation_cannot_regress_newer_readiness() {
        let state = AppState::new();
        state
            .interactions
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Booting, None)
            .await;

        record_provider_input_from_status_state(&state, "agent-1", "Idle", 2).await;
        record_provider_input_from_status_state(&state, "agent-1", "Processing...", 1).await;

        let current = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(current.state, ProviderInputReadiness::Ready);
        assert_eq!(current.generation, 1);
        assert_eq!(
            current.ready_evidence,
            Some(ProviderReadyEvidence::ProviderEvent)
        );
    }
}
