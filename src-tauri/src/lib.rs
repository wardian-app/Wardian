pub mod commands;
pub mod control;
pub mod manager;
pub mod providers;
pub mod remote;
pub mod state;
pub mod utils;
pub mod workflow;
pub use wardian_core::models;

// Tauri's Windows resource contains the Common Controls v6 manifest required by
// wry/tao imports such as TaskDialogIndirect. Cargo links it for app binaries,
// but the library unit-test harness needs the same resource explicitly.
#[cfg(all(test, windows))]
#[link(name = "resource", kind = "static")]
extern "C" {}

use crate::state::AppState;
use tauri::{Emitter, Manager};
use wardian_core::models::AgentConfig;

const TELEMETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const TELEMETRY_TICK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn restored_agent_without_process(
    config: AgentConfig,
    status: &str,
    output: String,
    process_id: Option<u32>,
    born: Option<String>,
) -> crate::state::ActiveAgent {
    let observed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut watch_state =
        crate::state::AgentWatchState::new(config.session_id.clone(), 4096, 262_144);
    watch_state.push_event(
        "status",
        serde_json::json!({
            "status": wardian_core::identity::normalize_status(status),
            "observed_at": observed_at,
        }),
    );

    crate::state::ActiveAgent {
        config: std::sync::Arc::new(std::sync::Mutex::new(config)),
        child_process: None,
        background_processes: Vec::new(),
        pty_master: None,
        stdin_tx: None,
        output_buffer: std::sync::Arc::new(std::sync::Mutex::new(output)),
        process_id,
        query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
        init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(born)),
        current_status: std::sync::Arc::new(std::sync::Mutex::new(status.to_string())),
        last_status_at: std::sync::Arc::new(std::sync::Mutex::new(Some(observed_at))),
        watch_state: std::sync::Arc::new(std::sync::Mutex::new(watch_state)),
        terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
        last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
        log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
        log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
        #[cfg(windows)]
        job_object: None,
    }
}

pub async fn reconcile_headless_agents() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use sysinfo::System;

    // Use new_all() to ensure we have process and environment data
    let mut sys = System::new_all();
    sys.refresh_all();

    let agents = wardian_core::db::get_all_agents()?;
    for agent in agents {
        if agent.is_off {
            continue;
        }

        let mut found_alive = false;
        for process in sys.processes().values() {
            for env_var in process.environ() {
                let env_str = env_var.to_string_lossy();
                if env_str.contains("WARDIAN_SESSION_ID=") && env_str.contains(&agent.session_id) {
                    found_alive = true;
                    let _ = wardian_core::db::update_agent_status(
                        &agent.session_id,
                        "Headless",
                        Some(process.pid().as_u32()),
                    );
                    break;
                }
            }
            if found_alive {
                break;
            }
        }

        if !found_alive && agent.last_status.as_deref() != Some("Off") {
            let _ = wardian_core::db::update_agent_status(&agent.session_id, "Off", None);
        }
    }
    Ok(())
}

async fn emit_metrics_tick(metrics_handle: tauri::AppHandle) {
    let state = metrics_handle.state::<AppState>();
    let metrics = manager::get_all_metrics(&state).await;
    let app_metrics = manager::get_app_metrics(&state).await;
    let _ = metrics_handle.emit("agent-metrics", &metrics);
    let _ = metrics_handle.emit("app-metrics", &app_metrics);
}

fn start_metrics_supervisor(metrics_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TELEMETRY_INTERVAL).await;
            let tick_handle = metrics_handle.clone();
            let mut tick = tauri::async_runtime::spawn(async move {
                emit_metrics_tick(tick_handle).await;
            });
            match tokio::time::timeout(TELEMETRY_TICK_TIMEOUT, &mut tick).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Telemetry tick failed; continuing metrics supervisor: {error}"
                    ));
                }
                Err(_) => {
                    tick.abort();
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Telemetry tick exceeded {}s; continuing metrics supervisor while timed-out work finishes in background",
                        TELEMETRY_TICK_TIMEOUT.as_secs()
                    ));
                }
            }
        }
    });
}

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

    crate::utils::fs::ensure_process_wardian_home_env();

    crate::utils::migration::migrate_home_layout();

    let db_init_result = crate::utils::fs::get_wardian_home()
        .map(|home| home.join("state.db"))
        .ok_or_else(|| "Could not resolve Wardian home".to_string())
        .and_then(|path| wardian_core::db::init_db_at_path(&path).map_err(|err| err.to_string()));
    if let Err(e) = db_init_result {
        eprintln!("Failed to initialize database: {}", e);
    }

    #[cfg(windows)]
    if let Err(e) = crate::utils::process::init_app_process_supervisor() {
        eprintln!("Failed to initialize process supervisor: {}", e);
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init());

    let builder = register_update_plugins(builder);

    builder
        .manage(AppState::new())
        .setup(|app| {
            {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.interactions.hydrate_from_persistence().await;
                });
            }

            let control_endpoint_claim = control::claim_control_endpoint().map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("Wardian control endpoint is already owned for this WARDIAN_HOME: {error}"),
                )
            })?;

            if let Ok(resource_dir) = app.path().resource_dir() {
                if let Some(app_home) = crate::utils::fs::get_wardian_home() {
                    if let Err(err) = crate::utils::cli_install::install_cli_from_resources_to_home(
                        &resource_dir,
                        &app_home,
                    ) {
                        crate::utils::logging::log_debug(&format!(
                            "[Wardian] CLI install skipped: {err}"
                        ));
                    }
                }
            }

            let app_handle = app.handle().clone();
            control::spawn_control_server(app_handle.clone(), control_endpoint_claim);
            remote::gateway::spawn_remote_gateway(app_handle.clone());
            manager::init_agent_classes(&app_handle);

            start_metrics_supervisor(app.handle().clone());

            if let Some(runs_dir) = wardian_core::paths::workflow_runs_dir() {
                let interrupted = crate::workflow::runs::scan_interrupted_runs(&runs_dir);
                if !interrupted.is_empty() {
                    crate::utils::logging::log_debug(&format!(
                        "[workflow] {} interrupted run(s) on startup: {:?}",
                        interrupted.len(),
                        interrupted
                    ));
                }
            }

            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();
                #[cfg(windows)]
                manager::cleanup_stale_persisted_session_processes();

                if let Err(e) = reconcile_headless_agents().await {
                    eprintln!("Failed to reconcile headless agents: {}", e);
                }
                crate::workflow::schedule::start_scheduler(app_handle.clone()).await;

                if let Some(app_dir) = manager::get_wardian_home() {
                    let state_path = app_dir.join("settings/state.json");
                    if let Ok(data) = std::fs::read_to_string(state_path) {
                        if let Ok(configs) = serde_json::from_str::<Vec<AgentConfig>>(&data) {
                            let mut agents_map = state.agents.lock().await;
                            let mut order_map = state.agent_order.lock().await;
                            let mut seen_names = std::collections::HashSet::new();
                            // Fetch latest status from DB for all agents
                            let db_agents = wardian_core::db::get_all_agents().unwrap_or_default();
                            type DbStatus = (Option<String>, Option<u32>, Option<String>);
                            let db_status_map: std::collections::HashMap<String, DbStatus> =
                                db_agents
                                    .into_iter()
                                    .map(|a| {
                                        (a.session_id, (a.last_status, a.last_pid, a.created_at))
                                    })
                                    .collect();

                            for mut config in configs {
                                // Sanitize name
                                let mut sanitized_name = config
                                    .session_name
                                    .chars()
                                    .map(|c| {
                                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                            c
                                        } else {
                                            '-'
                                        }
                                    })
                                    .collect::<String>();
                                if sanitized_name.is_empty() {
                                    sanitized_name = "agent".to_string();
                                }
                                let base_name = sanitized_name.clone();
                                let mut counter = 1;
                                while seen_names.contains(&sanitized_name) {
                                    sanitized_name = format!("{}-{}", base_name, counter);
                                    counter += 1;
                                }
                                seen_names.insert(sanitized_name.clone());
                                config.session_name = sanitized_name;

                                config.system_include_directories =
                                    Some(utils::fs::resolve_system_include_directories(
                                        &config.agent_class,
                                        &config.session_id,
                                    ));
                                if let Err(error) =
                                    commands::agent::prepare_restored_config_for_spawn(&mut config)
                                {
                                    eprintln!(
                                        "Failed to prepare restored agent {}: {}",
                                        config.session_id, error
                                    );
                                    continue;
                                }

                                let (last_status, last_pid, last_born) = db_status_map
                                    .get(&config.session_id)
                                    .cloned()
                                    .unwrap_or((None, None, None));

                                if last_status.as_deref() == Some("Headless") {
                                    let agent = restored_agent_without_process(
                                        config.clone(),
                                        "Headless",
                                        String::new(),
                                        last_pid,
                                        last_born,
                                    );
                                    order_map.push(config.session_id.clone());
                                    agents_map.insert(config.session_id.clone(), agent);
                                } else {
                                    let spawn_result = manager::spawn_agent(
                                        app_handle.clone(),
                                        config.clone(),
                                        true,
                                        last_born.clone(),
                                    )
                                    .await;
                                    match spawn_result {
                                        Ok(agent) => {
                                            if let Some(ref tx) = agent.stdin_tx {
                                                if let Ok(mut senders) = state.input_senders.write()
                                                {
                                                    senders.insert(
                                                        config.session_id.clone(),
                                                        tx.clone(),
                                                    );
                                                }
                                            }
                                            order_map.push(config.session_id.clone());
                                            agents_map.insert(config.session_id.clone(), agent);
                                        }
                                        Err(error) => {
                                            eprintln!(
                                                "Failed to restore agent {}: {}",
                                                config.session_id, error
                                            );
                                            let _ = wardian_core::db::update_agent_status(
                                                &config.session_id,
                                                "Error",
                                                None,
                                            );
                                            let agent = restored_agent_without_process(
                                                config.clone(),
                                                "Error",
                                                format!(
                                                    "Wardian could not restore this agent because its provider could not be launched.\r\n{}\r\n",
                                                    error
                                                ),
                                                None,
                                                last_born,
                                            );
                                            order_map.push(config.session_id.clone());
                                            agents_map.insert(config.session_id.clone(), agent);
                                        }
                                    }
                                }
                            }
                            manager::save_state(&app_handle, &agents_map, &order_map);
                            let _ = app_handle.emit("agents-updated", ());
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::agent::spawn_agent,
            commands::agent::list_provider_readiness,
            commands::agent::clone_agent,
            commands::agent::get_agent_clone_preview,
            commands::agent::list_agents,
            commands::agent::list_agent_metrics,
            commands::agent::kill_agent,
            commands::agent::pause_agent,
            commands::agent::resume_agent,
            commands::agent::clear_agent_session,
            commands::agent::rename_agent,
            commands::agent::reorder_agents,
            commands::agent::update_agent_config,
            commands::agent::build_agent_cli_command,
            commands::agent::enable_agent_worktree,
            commands::agent::list_agent_worktrees,
            commands::agent::assign_agent_worktree,
            commands::agent::delete_agent_worktree,
            commands::agent::disable_agent_worktree,
            commands::chat::load_agent_chat_transcript,
            commands::debug::debug_remove_agent_input_sender,
            commands::debug::debug_push_agent_watch_output,
            commands::debug::debug_set_agent_status,
            commands::terminal::send_input_to_agent,
            commands::terminal::submit_prompt_to_agent,
            commands::terminal::send_binary_input_to_agent,
            commands::terminal::inject_session_input,
            commands::terminal::broadcast_input,
            commands::terminal::resize_agent_terminal,
            commands::terminal::read_agent_pty,
            commands::terminal::ensure_user_terminal,
            commands::terminal::send_input_to_user_terminal,
            commands::terminal::send_binary_input_to_user_terminal,
            commands::terminal::resize_user_terminal,
            commands::terminal::read_user_terminal_pty,
            commands::terminal::restart_user_terminal,
            commands::terminal::set_user_terminal_cwd,
            commands::class::list_agent_classes,
            commands::class::create_agent_class,
            commands::class::delete_agent_class,
            commands::class::get_default_class_instruction,
            commands::class::reset_class_to_default,
            commands::class::reset_all_class_prompts,
            commands::fs::resolve_system_include_directories,
            commands::fs::validate_directory_path,
            commands::fs::get_explorer_root,
            commands::fs::get_directory_tree,
            commands::fs::delete_file,
            commands::fs::reveal_in_explorer,
            commands::fs::open_in_external_editor,
            commands::fs::terminal_link_target_exists,
            commands::fs::read_file_preview,
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
            commands::git::git_remove_worktree,
            commands::git::git_watch,
            commands::git::git_unwatch,
            commands::watchlist::load_watchlists,
            commands::watchlist::save_watchlists,
            commands::watchlist::load_watchlist_prefs,
            commands::watchlist::save_watchlist_prefs,
            commands::watchlist::load_queue_items,
            commands::watchlist::save_queue_items,
            commands::watchlist::load_queue_preferences,
            commands::watchlist::save_queue_preferences,
            commands::watchlist::load_opencode_last_assistant_text,
            commands::watchlist::load_agent_interactions,
            commands::watchlist::save_agent_interactions,
            commands::workflow::workflow_parse,
            commands::workflow::workflow_validate,
            commands::workflow::workflow_write,
            commands::workflow::workflow_list_blueprints,
            commands::workflow::workflow_list_runs,
            commands::workflow::workflow_read_run,
            commands::workflow::workflow_run,
            commands::workflow::workflow_resume,
            commands::workflow::workflow_approve,
            commands::workflow::workflow_cancel,
            commands::workflow::schedule_create,
            commands::workflow::schedule_list,
            commands::workflow::schedule_pause,
            commands::workflow::schedule_resume,
            commands::workflow::schedule_remove,
            commands::workflow::schedule_run_now,
            commands::library::get_library_tree,
            commands::library::save_library_item,
            commands::library::update_library_metadata,
            commands::library::open_library_folder,
            commands::library::deploy_skill,
            commands::library::remove_deployed_skill,
            commands::library::list_deployed_skills,
            commands::library::list_deployed_skill_refs,
            commands::library::list_skill_deployments,
            commands::library::library_watch,
            commands::library::library_unwatch,
            commands::patch::run_gemini_patch,
            commands::remote::approve_remote_pairing_request,
            commands::remote::create_remote_pairing_offer,
            commands::remote::list_pending_remote_pairing_requests,
            commands::remote::list_remote_devices,
            commands::remote::load_remote_access_status,
            commands::remote::load_remote_gateway_config,
            commands::remote::load_remote_setup_check,
            commands::remote::reject_remote_pairing_request,
            commands::remote::revoke_remote_device,
            commands::remote::save_remote_gateway_config,
            #[cfg(debug_assertions)]
            commands::remote::debug_create_remote_session,
            commands::settings::load_shell_settings,
            commands::settings::get_settings_folder_path,
            commands::settings::get_update_eligibility,
            commands::settings::install_update_with_windows_handoff,
            commands::settings::list_available_shells,
            commands::settings::load_app_settings,
            commands::settings::save_app_settings,
            commands::settings::save_shell_settings,
            commands::settings::save_agent_session_persistence,
            commands::settings::sync_provider_theme_settings,
            commands::settings::load_onboarding_hints,
            commands::settings::dismiss_onboarding_hint,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn register_update_plugins<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    if commands::settings::update_plugins_enabled_for_current_build() {
        builder
            .plugin(tauri_plugin_process::init())
            .plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn register_update_plugins<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
}
