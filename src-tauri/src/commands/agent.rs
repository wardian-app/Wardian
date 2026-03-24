use crate::manager;
use crate::models::AgentConfig;
use crate::state::AppState;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn spawn_agent(
    session_name: String,
    agent_class: String,
    folder: String,
    resume_session: Option<String>,
    is_off: Option<bool>,
    config_override: Option<AgentConfig>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AgentConfig, String> {
    manager::log_debug(&format!(
        "[WARDIAN] spawn_agent called for session name: {}, class: {}",
        session_name, agent_class
    ));
    let mut actual_resume = resume_session.clone().filter(|s| !s.is_empty());

    let mut session_id = actual_resume.clone();

    if actual_resume.is_none() {
        let cwd = if folder.is_empty() {
            if cfg!(windows) {
                std::env::var("USERPROFILE")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("C:"))
            } else {
                std::env::var("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("/"))
            }
        } else {
            std::path::PathBuf::from(&folder)
        };

        let provider_name = config_override.as_ref().map(|c| c.provider.clone()).unwrap_or_else(|| "gemini".to_string());
        if let Some(real_sid) = manager::obtain_session_id(&cwd, &provider_name).await {
            manager::log_debug(&format!(
                "[WARDIAN] Intercepted stream-json session ID for {}: {}",
                provider_name, real_sid
            ));
            // Properly set final_resume because manager::spawn_agent requires it to launch the persistent agent with --resume
            session_id = Some(real_sid.clone());
            actual_resume = Some(real_sid);
        } else {
            return Err("Failed to obtain session ID headlessly. Ensure the prompt \"Introduce yourself\" can execute.".to_string());
        }
    }

    let session_id = session_id.ok_or_else(|| "Failed to determine session ID".to_string())?;

    let mut config = config_override.unwrap_or_default();
    config.session_id = session_id.clone();
    config.session_name = session_name;
    config.agent_class = agent_class.clone();
    config.folder = folder;
    config.resume_session = actual_resume;
    config.is_off = is_off.unwrap_or(false);
    config.system_include_directories = Some(crate::utils::fs::resolve_system_include_directories(
        &agent_class,
        &session_id,
    ));

    let active_agent = manager::spawn_agent(app.clone(), config.clone(), false).await?;

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
    if let Some(mut agent) = agents.remove(&session_id) {
        // Remove from input_senders immediately
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        order.retain(|id| id != &session_id);
        manager::save_state(&app, &agents, &order);
        if let Some(mut child) = agent.child_process {
            let _ = child.kill();
        }
        #[cfg(windows)]
        {
            // Explicitly drop the job object to ensure all processes in the tree (including PTY hosts) are killed.
            let _ = agent.job_object.take();
        }

        // Cleanup: remove the agent's private directory
        if let Some(home) = crate::utils::fs::get_wardian_home() {
            let agent_dir = home.join("agents").join(&session_id);
            if agent_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&agent_dir) {
                    manager::log_debug(&format!("[WARDIAN] Failed to remove agent directory {:?}: {}", agent_dir, e));
                } else {
                    manager::log_debug(&format!("[WARDIAN] Successfully removed agent directory {:?}", agent_dir));
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
        if let Some(mut child) = agent.child_process.take() {
            let _ = child.kill();
        }

        #[cfg(windows)]
        {
            let _ = agent.job_object.take();
        }

        agent.pty_master = None;
        agent.stdin_tx = None;
        agent.process_id = None;
        agent.config.is_off = true;
        // Remove from input_senders
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        if let Ok(mut status) = agent.current_status.lock() {
            *status = "Off".to_string();
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
        agent.config.is_off = false;
        // Ensure resume_session is set so gemini CLI resumes the correct conversation
        if agent.config.resume_session.is_none() {
            agent.config.resume_session = Some(agent.config.session_id.clone());
        }
        let new_active = manager::spawn_agent(app.clone(), agent.config.clone(), true).await?;

        // Register new input sender
        if let Some(ref tx) = new_active.stdin_tx {
            if let Ok(mut senders) = state.input_senders.write() {
                senders.insert(session_id.clone(), tx.clone());
            }
        }

        // Replace ALL fields so the reader/writer threads share state with the stored agent
        agent.child_process = new_active.child_process;
        agent.pty_master = new_active.pty_master;
        agent.stdin_tx = new_active.stdin_tx;
        agent.process_id = new_active.process_id;
        agent.output_buffer = new_active.output_buffer;
        agent.query_count = new_active.query_count;
        agent.init_timestamp = new_active.init_timestamp;
        agent.current_status = new_active.current_status;
        agent.log_path = new_active.log_path;
        #[cfg(windows)]
        {
            agent.job_object = new_active.job_object;
        }
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
            new_config.system_include_directories = Some(
                crate::utils::fs::resolve_system_include_directories(&new_config.agent_class, &new_config.session_id),
            );
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
