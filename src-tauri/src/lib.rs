pub mod manager;
pub mod models;
pub mod state;
pub mod utils;
pub mod commands;
pub mod workflow_engine;

use tauri::{Emitter, Listener, Manager};
use crate::models::AgentConfig;
use crate::state::AppState;

#[derive(serde::Deserialize, Clone)]
struct TerminalInputPayload {
    #[serde(rename = "sessionId")]
    session_id: String,
    input: String,
}

/// The main entry point for the Wardian Tauri application.
/// Initializes plugins, state, and registers command handlers.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        use windows::core::PCWSTR;
        let aumid: Vec<u16> = "org.wardian.app"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = SetCurrentProcessExplicitAppUserModelID(PCWSTR::from_raw(aumid.as_ptr()));
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new())
        .setup(|app| {
            let app_handle = app.handle().clone();

            // Generate Class Constraints globally inside AppData
            manager::init_agent_classes(&app_handle);

            // Terminal input via Tauri events
            let event_handle = app.handle().clone();
            app.listen_any("terminal-input", move |event| {
                let raw = event.payload();
                if let Ok(payload) = serde_json::from_str::<TerminalInputPayload>(raw) {
                    if let Ok(senders) = event_handle.state::<AppState>().input_senders.try_read() {
                        if let Some(tx) = senders.get(&payload.session_id) {
                            let _ = tx.try_send(payload.input);
                        }
                    }
                }
            });

            // Metrics push task
            let metrics_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let state = metrics_handle.state::<AppState>();
                    let metrics = manager::get_all_metrics(&state).await;
                    let _ = metrics_handle.emit("agent-metrics", &metrics);
                }
            });

            // Restore agents from state
            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();
                
                // Initialize Workflow Triggers
                workflow_engine::init_triggers(app_handle.clone()).await;

                if let Some(app_dir) = manager::get_wardian_home() {
                    let state_path = app_dir.join("wardian_state.json");
                    if let Ok(data) = std::fs::read_to_string(state_path) {
                        if let Ok(configs) = serde_json::from_str::<Vec<AgentConfig>>(&data) {
                            let mut agents_map = state.agents.lock().await;
                            let mut order_map = state.agent_order.lock().await;
                            for config in configs {
                                if let Ok(agent) = manager::spawn_gemini_cli(app_handle.clone(), config.clone(), true).await {
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
            commands::agent::spawn_agent,
            commands::agent::list_agents,
            commands::agent::kill_agent,
            commands::agent::pause_agent,
            commands::agent::resume_agent,
            commands::agent::rename_agent,
            commands::agent::reorder_agents,
            commands::agent::update_agent_config,
            commands::terminal::send_input_to_agent,
            commands::terminal::inject_session_input,
            commands::terminal::broadcast_input,
            commands::terminal::resize_agent_terminal,
            commands::terminal::read_agent_pty,
            commands::class::list_agent_classes,
            commands::class::create_agent_class,
            commands::class::delete_agent_class,
            commands::watchlist::load_watchlists,
            commands::watchlist::save_watchlists,
            commands::fs::resolve_system_include_directories,
            commands::fs::validate_directory_path,
            commands::workflow::list_workflows,
            commands::workflow::save_workflow,
            commands::workflow::delete_workflow,
            commands::workflow::run_workflow,
            commands::workflow::stop_all_triggers,
            commands::workflow::pause_all_triggers,
            commands::workflow::resume_all_triggers,
            commands::workflow::load_workflow_library,
            commands::workflow::save_workflow_library,
            commands::library::get_library_tree,
            commands::library::save_library_item,
            commands::library::update_library_metadata,
            commands::library::open_library_folder,
            commands::library::deploy_skill,
            commands::library::remove_deployed_skill,
            commands::library::list_deployed_skills,
            commands::patch::run_gemini_patch
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

