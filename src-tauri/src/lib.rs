pub mod manager;

use manager::{AgentClassDefinition, AgentConfig, AppState};
use tauri::{AppHandle, Emitter, Listener, Manager, State};
use uuid::Uuid;

#[derive(serde::Deserialize, Clone)]
struct TerminalInputPayload {
    #[serde(rename = "sessionId")]
    session_id: String,
    input: String,
}
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
    // Use try_read to never block the async executor, even for a microsecond.
    let senders = match state.input_senders.try_read() {
        Ok(s) => s,
        Err(_) => {
            manager::log_debug(&format!(
                "[Wardian] [{}] send_input_to_agent: input_senders write-locked, dropping keystroke",
                session_id
            ));
            return Err("Input channel temporarily locked".to_string());
        }
    };
    if let Some(tx) = senders.get(&session_id) {
        match tx.try_send(input) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                manager::log_debug(&format!(
                    "[Wardian] [{}] send_input_to_agent: channel FULL (writer thread likely blocked on ConPTY write_all)",
                    session_id
                ));
                Err("Terminal input buffer full - PTY may be stalled".to_string())
            }
            Err(e) => {
                manager::log_debug(&format!(
                    "[Wardian] [{}] send_input_to_agent: channel error: {}",
                    session_id, e
                ));
                Err(format!("Failed to send input: {}", e))
            }
        }
    } else {
        Err(format!("Agent {} not found or is off", session_id))
    }
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

        #[cfg(windows)]
        {
            // Explicitly take and drop the job object. The Drop impl of win32job::Job
            // is what actually enforces the limit_kill_on_job_close() flag.
            // If we don't drop it here, orphaned conhost.exe processes will leak and drain CPU.
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
    let senders = state
        .input_senders
        .read()
        .map_err(|e| format!("Lock poisoned: {}", e))?;
    for tx in senders.values() {
        let _ = tx.try_send(input.clone());
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
async fn read_agent_pty(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(&session_id) {
        if let Ok(mut buf) = agent.output_buffer.lock() {
            if buf.is_empty() {
                Ok(None)
            } else {
                // Drain-on-read: take all accumulated output and clear the buffer
                Ok(Some(std::mem::take(&mut *buf)))
            }
        } else {
            Ok(None)
        }
    } else {
        Err(format!("Agent {} not found", session_id))
    }
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
    if let Some(app_dir) = manager::get_wardian_home() {
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
    if let Some(app_dir) = manager::get_wardian_home() {
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
    if let Some(app_dir) = manager::get_wardian_home() {
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
    if let Some(app_dir) = manager::get_wardian_home() {
        let role_dir = app_dir.join("classes").join(&name);
        if role_dir.exists() {
            let _ = std::fs::remove_dir_all(&role_dir);
        }
    }

    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
async fn load_watchlists(_app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    if let Some(app_dir) = manager::get_wardian_home() {
        let path = app_dir.join("watchlists.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
            return Ok(parsed);
        }
    }
    Ok(Vec::new())
}

#[tauri::command]
async fn save_watchlists(
    watchlists: Vec<serde_json::Value>,
    _app: AppHandle,
) -> Result<(), String> {
    let app_dir =
        manager::get_wardian_home().ok_or_else(|| "Could not find home directory".to_string())?;
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

            // Terminal input via Tauri events — completely bypasses the invoke/command pipeline.
            // Events go through Tauri's event system which is independent of the Tokio command
            // dispatcher. This makes terminal input immune to command queue saturation.
            let event_handle = app.handle().clone();
            app.listen_any("terminal-input", move |event| {
                let raw = event.payload();
                match serde_json::from_str::<TerminalInputPayload>(raw) {
                    Ok(payload) => {
                        let state = event_handle.state::<AppState>();
                        let tx_clone = {
                            let senders = match state.input_senders.try_read() {
                                Ok(s) => s,
                                Err(_) => {
                                    eprintln!(
                                        "[Wardian] EVENT: input_senders LOCKED for {}",
                                        payload.session_id
                                    );
                                    return;
                                }
                            };
                            match senders.get(&payload.session_id) {
                                Some(tx) => tx.clone(),
                                None => {
                                    eprintln!(
                                        "[Wardian] EVENT: no sender for {}",
                                        payload.session_id
                                    );
                                    return;
                                }
                            }
                        };
                        match tx_clone.try_send(payload.input) {
                            Ok(()) => {}
                            Err(e) => {
                                eprintln!(
                                    "[Wardian] EVENT: try_send FAILED for {}: {}",
                                    payload.session_id, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[Wardian] EVENT: failed to parse payload: {} raw={}",
                            e, raw
                        );
                    }
                }
            });

            // Metrics push task — emits agent-metrics every 5 seconds via events.
            // This eliminates invoke from the metrics path entirely.
            let metrics_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let state = metrics_handle.state::<AppState>();
                    let metrics = manager::get_all_metrics(&state).await;
                    let _ = metrics_handle.emit("agent-metrics", &metrics);
                }
            });

            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();
                if let Some(app_dir) = manager::get_wardian_home() {
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
                                    // Register input sender
                                    if let Some(ref tx) = agent.stdin_tx {
                                        if let Ok(mut senders) = state.input_senders.write() {
                                            senders.insert(config.session_id.clone(), tx.clone());
                                        }
                                    }
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
            read_agent_pty,
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
