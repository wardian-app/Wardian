pub mod commands;
pub mod manager;
pub mod models;
pub mod providers;
pub mod state;
pub mod utils;
pub mod workflow_engine;

use crate::models::AgentConfig;
use crate::state::AppState;
use tauri::{Emitter, Listener, Manager};

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
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        let aumid: Vec<u16> = "org.wardian.desktop"
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
                    let is_interrupt = payload.input.contains('\u{3}');
                    if let Ok(senders) = event_handle.state::<AppState>().input_senders.try_read() {
                        if let Some(tx) = senders.get(&payload.session_id) {
                            let _ = tx.try_send(payload.input);
                        }
                    }
                    if is_interrupt {
                        let app_handle = event_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app_handle.state::<AppState>();
                            let agents = state.agents.lock().await;
                            if let Some(agent) = agents.get(&payload.session_id) {
                                if let Ok(mut status) = agent.current_status.lock() {
                                    *status = "Idle".to_string();
                                }
                                let _ = app_handle.emit(
                                    "agent-status-updated",
                                    serde_json::json!({
                                        "session_id": payload.session_id,
                                        "current_status": "Idle",
                                    }),
                                );
                            }
                        });
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
                            for mut config in configs {
                                // Retroactively ensure the private agent directory is included
                                config.system_include_directories =
                                    Some(utils::fs::resolve_system_include_directories(
                                        &config.agent_class,
                                        &config.session_id,
                                    ));

                                if let Ok(agent) =
                                    manager::spawn_agent(app_handle.clone(), config.clone(), true)
                                        .await
                                {
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
            commands::class::get_default_class_instruction,
            commands::class::reset_class_to_default,
            commands::class::reset_all_class_prompts,
            commands::watchlist::load_watchlists,
            commands::watchlist::save_watchlists,
            commands::fs::resolve_system_include_directories,
            commands::fs::validate_directory_path,
            commands::fs::get_explorer_root,
            commands::fs::get_directory_tree,
            commands::fs::delete_file,
            commands::fs::reveal_in_explorer,
            commands::fs::read_file_preview,
            commands::workflow::list_workflows,
            commands::workflow::save_workflow,
            commands::workflow::delete_workflow,
            commands::workflow::run_workflow,
            commands::workflow::stop_all_triggers,
            commands::workflow::stop_workflow_triggers,
            commands::workflow::stop_workflow_run,
            commands::workflow::run_scheduled_workflow_now,
            commands::workflow::pause_all_triggers,
            commands::workflow::resume_all_triggers,
            commands::workflow::load_workflow_library,
            commands::workflow::save_workflow_library,
            commands::workflow::list_scheduled_runs,
            commands::workflow::create_scheduled_run,
            commands::workflow::delete_scheduled_run,
            commands::workflow::toggle_scheduled_run,
            commands::library::get_library_tree,
            commands::library::save_library_item,
            commands::library::update_library_metadata,
            commands::library::open_library_folder,
            commands::library::deploy_skill,
            commands::library::remove_deployed_skill,
            commands::library::list_deployed_skills,
            commands::library::list_skill_deployments,
            commands::patch::run_gemini_patch,
            commands::settings::list_available_shells,
            commands::settings::load_shell_settings,
            commands::settings::save_shell_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
