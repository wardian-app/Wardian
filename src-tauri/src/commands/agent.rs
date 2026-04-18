use crate::manager;
use crate::models::{
    AgentConfig, AgentSessionPersistence, AgentSessionPersistenceOverride, AgentTelemetry,
};
use crate::state::AppState;
use tauri::{AppHandle, Emitter, State};

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
    matches!(provider_name, "claude" | "codex")
}

fn restore_runtime_state_after_resume(
    new_active: &mut crate::state::ActiveAgent,
    old_active: &crate::state::ActiveAgent,
) {
    if let (Ok(old_count), Ok(mut new_count)) =
        (old_active.query_count.lock(), new_active.query_count.lock())
    {
        *new_count = *old_count;
    }
    if let (Ok(old_ts), Ok(mut new_ts)) = (
        old_active.init_timestamp.lock(),
        new_active.init_timestamp.lock(),
    ) {
        *new_ts = old_ts.clone();
    }
    if let (Ok(old_path), Ok(mut new_path)) =
        (old_active.log_path.lock(), new_active.log_path.lock())
    {
        *new_path = old_path.clone();
    }
}

fn restore_query_count_after_clear(
    new_active: &mut crate::state::ActiveAgent,
    old_active: &crate::state::ActiveAgent,
) {
    if let (Ok(old_count), Ok(mut new_count)) =
        (old_active.query_count.lock(), new_active.query_count.lock())
    {
        *new_count = *old_count;
    }
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
    }

    if config.resume_session.is_none() {
        let should_fallback =
            config.provider != "opencode" || config.session_id.starts_with("ses_");
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
    if config.provider == "claude" {
        config.fresh_provider_session_id = Some(uuid::Uuid::new_v4().to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn spawn_agent(
    req: SpawnAgentRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AgentConfig, String> {
    let session_name = req.session_name;
    let agent_class = req.agent_class;
    let folder = req.folder;
    let resume_session = req.resume_session;
    let is_off = req.is_off;
    let config_override = req.config_override;

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
                    return Err(format!("Failed to initialize the provider session: {}", e));
                }
            }
        }
    }

    let session_id = session_id.ok_or_else(|| "Failed to determine session ID".to_string())?;

    let mut config = config_override.unwrap_or_default();
    config.session_id = session_id.clone();
    config.session_name = session_name;
    config.agent_class = agent_class.clone();
    config.folder = folder;
    config.resume_session = actual_resume.clone();
    config.is_off = is_off.unwrap_or(false);
    config.system_include_directories = Some(crate::utils::fs::resolve_system_include_directories(
        &agent_class,
        &session_id,
    ));
    let mut active_agent = manager::spawn_agent(app.clone(), config.clone(), false).await?;
    // Propagate any fields that spawn_agent may have auto-assigned (e.g. opencode_port).
    config.opencode_port = active_agent.config.opencode_port;
    if config.provider == "codex" && actual_resume.is_none() {
        for _ in 0..40 {
            if let Some((provider_session_id, _updated_at)) =
                manager::latest_codex_session_index_entry(&session_id)?
            {
                actual_resume = Some(provider_session_id.clone());
                config.resume_session = Some(provider_session_id.clone());
                active_agent.config.resume_session = Some(provider_session_id.clone());
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
    active_agent.config.resume_session = persisted_resume;

    // Register input sender BEFORE locking agents map
    if let Some(ref tx) = active_agent.stdin_tx {
        if let Ok(mut senders) = state.input_senders.write() {
            senders.insert(session_id.clone(), tx.clone());
        }
    }

    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    agents.insert(session_id.clone(), active_agent);
    order.push(session_id.clone());
    manager::save_state(&app, &agents, &order);

    Ok(config)
}

#[tauri::command]
pub async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentConfig>, String> {
    manager::log_debug("[WARDIAN] list_agents called");
    let agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    let mut list: Vec<AgentConfig> = Vec::new();
    for id in order.iter() {
        if let Some(agent) = agents.get(id) {
            list.push(agent.config.clone());
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
        // The log file is populated by the metrics poll loop; if no session
        // was ever created (user paused before sending a message) this is a
        // no-op and resume will simply start a fresh opencode session.
        if agent.config.provider == "opencode"
            && agent
                .config
                .resume_session
                .as_deref()
                .map(|s| !s.starts_with("ses_"))
                .unwrap_or(true)
        {
            let log_path_snap = agent.log_path.lock().ok().and_then(|guard| guard.clone());
            if let Some(log_path) = log_path_snap {
                if let Some(ses_id) = manager::opencode_extract_created_session_id(&log_path) {
                    agent.config.resume_session = Some(ses_id);
                }
            }
        }

        agent.pty_master = None;
        agent.stdin_tx = None;
        agent.config.is_off = true;
        // Remove from input_senders
        if let Ok(mut status) = agent.current_status.lock() {
            *status = "Off".to_string();
        }
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        manager::save_state(&app, &agents, &order);
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
    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;

    if let Some(agent) = agents.get_mut(&session_id) {
        prepare_resume_config(&mut agent.config)?;
        let mut new_active = manager::spawn_agent(app.clone(), agent.config.clone(), true).await?;
        restore_runtime_state_after_resume(&mut new_active, agent);
        if agent.config.provider == "claude" {
            if let Some(fresh_provider_session_id) = agent.config.fresh_provider_session_id.take() {
                agent.config.resume_session = Some(fresh_provider_session_id);
            }
        }

        // Register new input sender
        if let Some(ref tx) = new_active.stdin_tx {
            if let Ok(mut senders) = state.input_senders.write() {
                senders.insert(session_id.clone(), tx.clone());
            }
        }

        // Terminate the old agent's process tree before replacing it.
        manager::terminate_active_agent_process(agent);

        // Preserve the updated config on the new agent, then swap the entire struct.
        // The old ActiveAgent is dropped here; its Drop impl is a no-op because
        // terminate_active_agent_process already cleared process_id and child_process.
        new_active.config = agent.config.clone();
        let _ = std::mem::replace(agent, new_active);
        manager::save_state(&app, &agents, &order);
        Ok(())
    } else {
        Err(format!("Agent {} not found", session_id))
    }
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
        prepare_clear_config(&mut agent.config)?;
        if let Ok(mut buf) = agent.output_buffer.lock() {
            buf.clear();
        }
        if let Ok(mut title) = agent.terminal_title.lock() {
            title.clear();
        }
        if let Ok(mut status) = agent.current_status.lock() {
            *status = "Processing...".to_string();
        }
        let _ = app.emit(
            "agent-terminal-cleared",
            serde_json::json!({ "session_id": session_id }),
        );

        let mut new_active = manager::spawn_agent(app.clone(), agent.config.clone(), true).await?;
        restore_query_count_after_clear(&mut new_active, agent);
        if agent.config.provider == "claude" {
            if let Some(fresh_provider_session_id) = agent.config.fresh_provider_session_id.take() {
                agent.config.resume_session = Some(fresh_provider_session_id);
            }
        }

        if let Some(ref tx) = new_active.stdin_tx {
            if let Ok(mut senders) = state.input_senders.write() {
                senders.insert(session_id.clone(), tx.clone());
            }
        }

        manager::terminate_active_agent_process(agent);
        new_active.config = agent.config.clone();
        let _ = std::mem::replace(agent, new_active);
        manager::save_state(&app, &agents, &order);
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
    let mut agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;

    if let Some(agent) = agents.get_mut(&session_id) {
        agent.config.session_name = new_name;
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
        if agent.config.agent_class != new_config.agent_class {
            manager::log_debug(&format!(
                "[WARDIAN] Agent class changed from {} to {}. Updating system include directories.",
                agent.config.agent_class, new_config.agent_class
            ));
            new_config.system_include_directories =
                Some(crate::utils::fs::resolve_system_include_directories(
                    &new_config.agent_class,
                    &new_config.session_id,
                ));
        }

        agent.config = new_config.clone();
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
        persisted_resume_session_for_provider, prepare_clear_config, prepare_resume_config,
        provider_uses_generated_session_id, restore_query_count_after_clear,
        restore_runtime_state_after_resume,
    };
    use crate::models::{AgentConfig, AgentSessionPersistenceOverride};
    use crate::state::ActiveAgent;
    use std::sync::{Arc, Mutex};

    fn make_test_agent() -> ActiveAgent {
        ActiveAgent {
            config: AgentConfig::default(),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: Arc::new(Mutex::new(String::new())),
            process_id: None,
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Idle".to_string())),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    fn use_isolated_resume_setting() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: crate::models::AgentSessionPersistence::Resume,
            ..Default::default()
        })
        .expect("save shell settings");
        (guard, temp)
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
    fn resume_restores_query_count_and_log_path() {
        let old_active = make_test_agent();
        *old_active.query_count.lock().unwrap() = 3;
        *old_active.init_timestamp.lock().unwrap() = Some("2026-04-12T17:00:00.000Z".to_string());
        *old_active.log_path.lock().unwrap() =
            Some(std::path::PathBuf::from("C:/tmp/session.json"));

        let mut new_active = make_test_agent();
        restore_runtime_state_after_resume(&mut new_active, &old_active);

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
    fn clear_preserves_query_count_only() {
        let old_active = make_test_agent();
        *old_active.query_count.lock().unwrap() = 7;
        *old_active.init_timestamp.lock().unwrap() = Some("2026-04-12T17:00:00.000Z".to_string());
        *old_active.log_path.lock().unwrap() =
            Some(std::path::PathBuf::from("C:/tmp/session.json"));

        let mut new_active = make_test_agent();
        restore_query_count_after_clear(&mut new_active, &old_active);

        assert_eq!(*new_active.query_count.lock().unwrap(), 7);
        assert_eq!(new_active.init_timestamp.lock().unwrap().as_deref(), None);
        assert_eq!(new_active.log_path.lock().unwrap().as_deref(), None);
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
    fn global_fresh_session_persistence_resume_clears_resume_session() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: crate::models::AgentSessionPersistence::Fresh,
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
            agent_session_persistence: crate::models::AgentSessionPersistence::Resume,
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
            agent_session_persistence: crate::models::AgentSessionPersistence::Fresh,
            ..Default::default()
        })
        .expect("save shell settings");

        let mut config = AgentConfig {
            provider: "gemini".to_string(),
            session_id: "gemini-session".to_string(),
            resume_session: None,
            session_persistence: AgentSessionPersistenceOverride::Resume,
            is_off: true,
            ..Default::default()
        };

        prepare_resume_config(&mut config).expect("prepare resume config");

        assert_eq!(config.resume_session.as_deref(), Some("gemini-session"));
        assert!(!config.is_off);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn claude_fresh_resume_uses_new_provider_session_without_changing_wardian_id() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            agent_session_persistence: crate::models::AgentSessionPersistence::Fresh,
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
