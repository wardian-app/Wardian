pub mod commands;
pub mod manager;
pub mod models;
pub mod providers;
pub mod state;
pub mod utils;
pub mod workflow_engine;

use crate::models::AgentConfig;
use crate::state::AppState;
use tauri::{Emitter, Manager};

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
            commands::agent::list_agent_metrics,
            commands::agent::kill_agent,
            commands::agent::pause_agent,
            commands::agent::resume_agent,
            commands::agent::rename_agent,
            commands::agent::reorder_agents,
            commands::agent::update_agent_config,
            commands::terminal::send_input_to_agent,
            commands::terminal::submit_prompt_to_agent,
            commands::terminal::send_binary_input_to_agent,
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
            commands::settings::save_shell_settings,
            commands::settings::save_opencode_theme,
            commands::git::git_status,
            commands::git::git_current_branch,
            commands::git::git_log,
            commands::git::git_diff_file,
            commands::git::git_stage,
            commands::git::git_unstage,
            commands::git::git_discard_changes,
            commands::git::git_commit,
            commands::git::git_pull,
            commands::git::git_push,
            commands::git::git_create_worktree,
            commands::git::git_remove_worktree
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();

                // Terminate all agent process trees on app exit to prevent zombies.
                // We use try_lock to avoid deadlocking with background tasks during shutdown.
                // If the lock is held, the agents will still be terminated when AppState
                // drops, which triggers the `Drop` safety net on `ActiveAgent`.
                {
                    if let Ok(mut agents) = state.agents.try_lock() {
                        for (_sid, agent) in agents.iter_mut() {
                            manager::terminate_active_agent_process(agent);
                        }
                        agents.clear();
                    }
                }

                // Abort all workflow triggers and running executions.
                {
                    if let Ok(mut triggers) = state.workflow_triggers.try_lock() {
                        for (_wf_id, handles) in triggers.drain() {
                            for handle in handles {
                                handle.abort();
                            }
                        }
                    }
                }
                {
                    if let Ok(mut runs) = state.workflow_runs.try_lock() {
                        for (_wf_id, handles) in runs.drain() {
                            for handle in handles {
                                handle.abort();
                            }
                        }
                    }
                }
                {
                    if let Ok(mut scheduler) = state.scheduler_handle.try_lock() {
                        if let Some(h) = scheduler.take() {
                            h.abort();
                        }
                    };
                }
            }
        });
}
