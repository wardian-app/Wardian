pub mod manager;

use manager::{AgentClassDefinition, AgentConfig, AppState};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
#[tauri::command]
async fn spawn_agent(
    session_name: String,
    agent_class: String,
    folder: String,
    resume_session: Option<String>,
    is_off: Option<bool>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AgentConfig, String> {
    manager::log_debug(&format!(
        "[WARDIAN] spawn_agent called for session name: {}, class: {}",
        session_name, agent_class
    ));
    let actual_resume = resume_session.clone().filter(|s| !s.is_empty());

    let mut session_id = actual_resume.clone().unwrap_or_else(|| String::new());
    let mut actual_resume_config = actual_resume.clone();

    if actual_resume.is_none() {
        let cwd = if folder.is_empty() {
            if cfg!(windows) {
                std::env::var("USERPROFILE")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("C:\\"))
            } else {
                std::env::var("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("/"))
            }
        } else {
            std::path::PathBuf::from(&folder)
        };

        if let Some(real_sid) = manager::obtain_session_id_headless(&cwd).await {
            manager::log_debug(&format!(
                "[WARDIAN] Intercepted stream-json session ID: {}",
                real_sid
            ));
            session_id = real_sid.clone();
            actual_resume_config = Some(real_sid);
        } else {
            session_id = Uuid::new_v4().to_string();
        }
    }

    let config = AgentConfig {
        session_id: session_id.clone(),
        session_name,
        agent_class,
        folder,
        resume_session: actual_resume_config,
        is_off: is_off.unwrap_or(false),
    };

    let active_agent = manager::spawn_gemini_cli(app.clone(), config.clone(), false).await?;

    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    agents.insert(session_id.clone(), active_agent);
    order.push(session_id.clone());
    manager::save_state(&app, &agents, &order);

    Ok(config)
}

#[tauri::command]
async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentConfig>, String> {
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
async fn send_input_to_agent(
    session_id: String,
    input: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] send_input_to_agent called for session {} with input {:?}",
        session_id, input
    ));
    let tx = {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&session_id) {
            if let Some(ref tx) = agent.stdin_tx {
                tx.clone()
            } else {
                return Err(format!("Agent {} is currently off", session_id));
            }
        } else {
            return Err(format!("Agent {} not found", session_id));
        }
    }; // Lock dropped here

    tx.send(input)
        .await
        .map_err(|e| format!("Failed to send input: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn kill_agent(
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
    if let Some(agent) = agents.remove(&session_id) {
        order.retain(|id| id != &session_id);
        manager::save_state(&app, &agents, &order);
        if let Some(mut child) = agent.child_process {
            let _ = child.kill();
        }
        Ok(())
    } else {
        let err_msg = format!("Agent with session ID {} not found", session_id);
        manager::log_debug(&format!("[WARDIAN] {}", err_msg));
        Err(err_msg)
    }
}

#[tauri::command]
async fn pause_agent(
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
        agent.pty_master = None;
        agent.stdin_tx = None;
        agent.process_id = None;
        agent.config.is_off = true;
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
async fn resume_agent(
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
        let new_active = manager::spawn_gemini_cli(app.clone(), agent.config.clone(), true).await?;

        // Replace ALL fields so the reader/writer threads share state with the stored agent
        agent.child_process = new_active.child_process;
        agent.pty_master = new_active.pty_master;
        agent.stdin_tx = new_active.stdin_tx;
        agent.process_id = new_active.process_id;
        agent.output_history = new_active.output_history;
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
async fn rename_agent(
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
async fn broadcast_input(input: String, state: State<'_, AppState>) -> Result<(), String> {
    manager::log_debug("[WARDIAN] broadcast_input called");
    let txs: Vec<_> = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .filter_map(|a| a.stdin_tx.as_ref().cloned())
            .collect()
    };
    for tx in txs {
        let _ = tx.send(input.clone()).await;
    }
    Ok(())
}

#[tauri::command]
async fn resize_agent_terminal(
    session_id: String,
    cols: u16,
    rows: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] resize_agent_terminal called for session {}",
        session_id
    ));
    manager::resize_pty(session_id, cols, rows, &state).await
}

#[tauri::command]
async fn attach_agent_pty(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] attach_agent_pty called for session {}",
        session_id
    ));
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(&session_id) {
        if let Ok(history) = agent.output_history.lock() {
            if !history.is_empty() {
                let _ = app.emit(
                    "agent-output",
                    serde_json::json!({
                        "session_id": session_id,
                        "text": history.as_str(),
                        "stream": "stdout"
                    }),
                );
            }
        }
    } else {
        return Err(format!("Agent {} not found", session_id));
    }
    Ok(())
}

#[tauri::command]
async fn reorder_agents(
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

#[tauri::command]
async fn get_agent_metrics(
    state: State<'_, AppState>,
) -> Result<Vec<manager::AgentTelemetry>, String> {
    Ok(manager::get_all_metrics(&state).await)
}

#[tauri::command]
async fn list_agent_classes(app: AppHandle) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug("[WARDIAN] list_agent_classes called");
    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
async fn create_agent_class(
    name: String,
    description: String,
    gemini_md: Option<String>,
    app: AppHandle,
) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug(&format!("[WARDIAN] create_agent_class called: {}", name));
    let trimmed_name = name.trim().to_string();
    if trimmed_name.is_empty() {
        return Err("Class name cannot be empty".to_string());
    }

    // Check for duplicate names across all classes
    let all = manager::get_all_agent_classes(&app);
    if all
        .iter()
        .any(|c| c.name.to_lowercase() == trimmed_name.to_lowercase())
    {
        return Err(format!("A class named '{}' already exists", trimmed_name));
    }

    // Read existing custom classes, append, save
    let mut custom: Vec<AgentClassDefinition> = Vec::new();
    if let Ok(app_dir) = app.path().app_data_dir() {
        let custom_path = app_dir.join("custom_classes.json");
        if let Ok(data) = std::fs::read_to_string(&custom_path) {
            if let Ok(parsed) = serde_json::from_str::<Vec<AgentClassDefinition>>(&data) {
                custom = parsed;
            }
        }
    }

    let new_class = AgentClassDefinition {
        name: trimmed_name.clone(),
        description: description.trim().to_string(),
        is_default: false,
    };

    // Save the new class first so init_agent_classes picks it up
    custom.push(new_class.clone());
    manager::save_custom_classes(&app, &custom)?;

    // Scaffold the new class directory
    if let Ok(app_dir) = app.path().app_data_dir() {
        let role_dir = app_dir.join("classes").join(&trimmed_name);
        let _ = std::fs::create_dir_all(&role_dir);
        let gemini_md_path = role_dir.join("GEMINI.md");
        if !gemini_md_path.exists() {
            let content = match gemini_md {
                Some(ref md) if !md.trim().is_empty() => md.clone(),
                _ => format!("# {} Agent\n\n{}\n", trimmed_name, new_class.description),
            };
            let _ = std::fs::write(gemini_md_path, content);
        }
    }

    // Regenerate AGENTS.md registry (now includes the new class)
    manager::init_agent_classes(&app);

    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
async fn delete_agent_class(
    name: String,
    app: AppHandle,
) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug(&format!("[WARDIAN] delete_agent_class called: {}", name));

    // Prevent deleting default classes
    let all = manager::get_all_agent_classes(&app);
    if let Some(found) = all.iter().find(|c| c.name == name) {
        if found.is_default {
            return Err("Cannot delete a default class".to_string());
        }
    } else {
        return Err(format!("Class '{}' not found", name));
    }

    // Read existing custom classes, remove, save
    let mut custom: Vec<AgentClassDefinition> = Vec::new();
    if let Ok(app_dir) = app.path().app_data_dir() {
        let custom_path = app_dir.join("custom_classes.json");
        if let Ok(data) = std::fs::read_to_string(&custom_path) {
            if let Ok(parsed) = serde_json::from_str::<Vec<AgentClassDefinition>>(&data) {
                custom = parsed;
            }
        }
    }

    custom.retain(|c| c.name != name);
    manager::save_custom_classes(&app, &custom)?;

    // Optionally remove the class directory
    if let Ok(app_dir) = app.path().app_data_dir() {
        let role_dir = app_dir.join("classes").join(&name);
        if role_dir.exists() {
            let _ = std::fs::remove_dir_all(&role_dir);
        }
    }

    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
async fn load_watchlists(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    use tauri::Manager;
    if let Ok(app_dir) = app.path().app_data_dir() {
        let path = app_dir.join("watchlists.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
            return Ok(parsed);
        }
    }
    Ok(Vec::new())
}

#[tauri::command]
async fn save_watchlists(watchlists: Vec<serde_json::Value>, app: AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let app_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&app_dir);
    let path = app_dir.join("watchlists.json");
    let json = serde_json::to_string_pretty(&watchlists).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .setup(|app| {
            let app_handle = app.handle().clone();

            // Generate Class Constraints globally inside AppData
            manager::init_agent_classes(&app_handle);

            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();
                if let Ok(app_dir) = app_handle.path().app_data_dir() {
                    let state_path = app_dir.join("wardian_state.json");
                    if let Ok(data) = std::fs::read_to_string(state_path) {
                        if let Ok(configs) = serde_json::from_str::<Vec<AgentConfig>>(&data) {
                            let mut agents_map = state.agents.lock().await;
                            let mut order_map = state.agent_order.lock().await;
                            for config in configs {
                                if let Ok(agent) = manager::spawn_gemini_cli(
                                    app_handle.clone(),
                                    config.clone(),
                                    true,
                                )
                                .await
                                {
                                    order_map.push(config.session_id.clone());
                                    agents_map.insert(config.session_id.clone(), agent);
                                }
                            }
                            let _ = app_handle.emit("agents-updated", ());
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            spawn_agent,
            list_agents,
            send_input_to_agent,
            kill_agent,
            pause_agent,
            resume_agent,
            broadcast_input,
            resize_agent_terminal,
            attach_agent_pty,
            get_agent_metrics,
            reorder_agents,
            rename_agent,
            list_agent_classes,
            create_agent_class,
            delete_agent_class,
            load_watchlists,
            save_watchlists
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<AppState>();
                tauri::async_runtime::block_on(manager::kill_all_agents(&state));
            }
        });
}
