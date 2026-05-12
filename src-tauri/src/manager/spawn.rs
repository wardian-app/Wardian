use crate::providers::claude::{classify_claude_user_event, ClaudeUserEventKind};
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AgentWatchState, AppState};
use crate::utils::fs::*;
use crate::utils::logging::{log_debug, log_terminal_trace_bytes, log_terminal_trace_note};
use crate::utils::PtyUtf8Decoder;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{BufRead, Read, Seek, Write};
use tauri::{AppHandle, Emitter};
use wardian_core::models::{AgentConfig, AgentEvent};

use super::claude::{claude_permission_hook_matches_session, claude_project_dir_name};
use super::codex::{
    codex_provider_session_is_excluded, codex_session_file_path, latest_codex_session_index_entry,
};
use super::opencode::{opencode_interactive_env, opencode_status_from_title};
use super::{
    apply_agent_event, apply_agent_status_event, apply_terminal_identity_env, debug_preview_bytes,
    extract_terminal_titles, finalize_interactive_spawn_args, interactive_provider_args,
    interactive_provider_cwd, interactive_provider_launch, set_agent_status,
};
use crate::providers::gemini::gemini_status_from_title;

#[cfg(target_os = "macos")]
use super::macos_extended_path;
#[cfg(windows)]
use super::{
    app_process_supervisor_active, assign_pid_to_job, cleanup_stale_session_processes,
    create_kill_on_close_job,
};

pub(super) fn capture_codex_init_resume_session(
    provider_name: &str,
    session_id: &str,
    config: &mut AgentConfig,
) -> bool {
    let session_id = session_id.trim();
    if provider_name != "codex"
        || session_id.is_empty()
        || config
            .resume_session
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || codex_provider_session_is_excluded(session_id, &config.codex_cleared_provider_sessions)
    {
        return false;
    }

    config.resume_session = Some(session_id.to_string());
    config.codex_cleared_provider_sessions.clear();
    true
}

fn codex_status_log_session(
    config: &mut AgentConfig,
    latest_session: Option<String>,
) -> Option<String> {
    if config.resume_session.as_deref().is_some_and(|value| {
        codex_provider_session_is_excluded(value, &config.codex_cleared_provider_sessions)
    }) {
        config.resume_session = None;
    }

    let candidate = config
        .resume_session
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| latest_session.filter(|value| !value.trim().is_empty()));

    let candidate = candidate?;

    if codex_provider_session_is_excluded(&candidate, &config.codex_cleared_provider_sessions) {
        return Some(candidate);
    }

    if config
        .resume_session
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        config.resume_session = Some(candidate.clone());
        config.codex_cleared_provider_sessions.clear();
    } else if config.resume_session.as_deref() == Some(candidate.as_str())
        && !config.codex_cleared_provider_sessions.is_empty()
    {
        config.codex_cleared_provider_sessions.clear();
    }

    Some(candidate)
}

fn save_agent_state_after_session_capture(app: &AppHandle) {
    use tauri::Manager;

    let app_state = app.state::<crate::state::app_state::AppState>();
    let agents = app_state.agents.blocking_lock();
    let order = app_state.agent_order.blocking_lock();
    crate::manager::save_state(app, &agents, &order);
}

pub async fn spawn_agent(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
    initial_timestamp: Option<String>,
) -> Result<ActiveAgent, String> {
    let provider = ProviderFactory::resolve(&config.provider)?;

    let cwd = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);

    let expected_folder = if config.folder.is_empty() {
        cwd.to_string_lossy().to_string()
    } else {
        config.folder.clone()
    };

    // Phase 2: Record/Update agent in SQLite with explicit ISO 8601 timestamp
    let born_to_save = initial_timestamp
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let project = wardian_core::db::project_name_from_workspace(&expected_folder);
    let _ = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &config.session_id,
        session_name: &config.session_name,
        agent_class: &config.agent_class,
        provider: &config.provider,
        workspace: Some(&expected_folder),
        project: project.as_deref(),
        is_off: config.is_off,
        created_at: Some(&born_to_save),
    });

    if config.is_off {
        let _ = wardian_core::db::update_agent_status(&config.session_id, "Off", None);
        let session_id = config.session_id.clone();

        return Ok(ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(config)),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(Some(born_to_save))),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Off".to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(AgentWatchState::new(
                session_id, 4096, 262_144,
            ))),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        });
    }

    let config_lock = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));

    #[cfg(windows)]
    cleanup_stale_session_processes(&config.session_id, &config.provider);

    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open pty: {}", e))?;

    let (bin, mut provider_args) = provider.get_executable();
    let claude_hook = if config.provider == "claude" {
        Some(ensure_claude_permission_hook(&config.session_id)?)
    } else {
        None
    };
    let habitat_root = prepare_provider_habitat(
        &config.provider,
        &cwd,
        &config.agent_class,
        Some(&config.session_id),
    )?;
    let provider_cwd =
        interactive_provider_cwd(&config.provider, &cwd, habitat_root.as_deref(), None);

    if config.provider == "claude" {
        if let Some(hook) = claude_hook.as_ref() {
            provider_args.push("--settings".to_string());
            provider_args.push(hook.settings_arg.clone());
        }
    }

    let background_processes = Vec::new();
    let is_resume = config
        .resume_session
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let spawn_args = provider.get_spawn_args(&config, is_resume);
    let spawn_args = finalize_interactive_spawn_args(
        &config.provider,
        is_restored,
        &config.resume_session,
        spawn_args,
    );
    provider_args.extend(spawn_args);
    provider_args = interactive_provider_args(&config.provider, &provider_cwd, &cwd, provider_args);

    let launch_spec = interactive_provider_launch(&config.provider, &bin, &provider_args)?;
    log_debug(&format!(
        "[Wardian] PTY spawn: provider={} exe={} args={:?} cwd={}",
        config.provider,
        launch_spec.executable,
        launch_spec.args,
        provider_cwd.display()
    ));
    let mut cmd = CommandBuilder::new(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }
    cmd.cwd(&provider_cwd);
    apply_terminal_identity_env(&mut cmd);
    cmd.env("WARDIAN_SESSION_ID", &config.session_id);

    // Enable CLAUDE.md discovery from --add-dir directories so that
    // class/common/agent instruction files are loaded natively.
    if config.provider == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
    } else if config.provider == "codex" {
        if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if config.provider == "opencode" {
        for (key, value) in opencode_interactive_env(&provider_cwd, &config)? {
            cmd.env(key, value);
        }
    }
    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    let resume_id = config.resume_session.as_deref().unwrap_or("");
    log_debug(&format!(
        "[Wardian] Spawning {} agent. Session: {}, Resume ID: {}, Restored: {}",
        provider.name(),
        config.session_id,
        resume_id,
        is_restored
    ));

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    let process_id = child.process_id();

    // Phase 2: Record/Update status in SQLite with the real PID
    let _ = wardian_core::db::update_agent_status(
        &config.session_id,
        if config.is_off { "Off" } else { "Idle" },
        process_id,
    );

    #[cfg(windows)]
    let job_object = {
        if app_process_supervisor_active() {
            None
        } else if let Ok(job) = create_kill_on_close_job("agent fallback") {
            if let Some(pid) = process_id {
                if let Err(err) = assign_pid_to_job(&job, pid, "agent fallback") {
                    log_debug(&format!(
                        "[Wardian] Failed to assign session {} PID {} to fallback job: {}",
                        config.session_id, pid, err
                    ));
                }
            }
            Some(job)
        } else {
            None
        }
    };
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get pty reader: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get pty writer: {}", e))?;
    let pty_master = std::sync::Arc::new(std::sync::Mutex::new(pair.master));
    drop(pair.slave);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let sid_for_input = config.session_id.clone();
    let provider_name_for_input = config.provider.clone();

    std::thread::spawn(move || {
        while let Some(input) = rx.blocking_recv() {
            if provider_name_for_input == "opencode" {
                log_debug(&format!(
                    "[Wardian] OpenCode PTY input for session {}: {}",
                    sid_for_input,
                    debug_preview_bytes(&input, 128)
                ));
            }
            log_terminal_trace_bytes(&sid_for_input, &provider_name_for_input, "IN", &input);
            let _ = writer.write_all(&input);
            let _ = writer.flush();
        }
        log_terminal_trace_note(
            &sid_for_input,
            &provider_name_for_input,
            "input channel closed",
        );
    });

    let sid_out = config.session_id.clone();
    let provider_name_for_pty = config.provider.clone();
    let output_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let output_buffer_clone = output_buffer.clone();
    let query_count = std::sync::Arc::new(std::sync::Mutex::new(0));
    let query_count_clone = query_count.clone();
    let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(Some(born_to_save)));
    let init_timestamp_clone = init_timestamp.clone();
    let current_status = std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string()));
    let current_status_clone = current_status.clone();
    let watch_state = std::sync::Arc::new(std::sync::Mutex::new(AgentWatchState::new(
        config.session_id.clone(),
        4096,
        262_144,
    )));
    let watch_state_clone = watch_state.clone();
    let terminal_title = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let terminal_title_clone = terminal_title.clone();
    let last_output_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let last_output_at_clone = last_output_at.clone();
    let log_path = std::sync::Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    // PTY reader thread: uses provider.parse_output() for event classification
    let pty_app = app.clone();
    let pty_provider = provider.clone();
    let sid_for_pty = sid_out.clone();
    let pty_emit_app = app.clone();
    let config_lock_clone = config_lock.clone();
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut current_line = String::new();
        let mut had_pty_output = false;
        let mut opencode_chunks_logged = 0usize;
        let mut pty_decoder = PtyUtf8Decoder::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    log_terminal_trace_note(&sid_for_pty, &provider_name_for_pty, "pty EOF");
                    if provider_name_for_pty == "opencode" {
                        log_debug(&format!(
                            "[Wardian] OpenCode PTY EOF for session {} (had_output={})",
                            sid_for_pty, had_pty_output
                        ));
                    }
                    // If the process exited immediately with no output, surface a
                    // diagnostic message so the terminal is not silently blank.
                    if !had_pty_output && provider_name_for_pty == "opencode" {
                        let msg = concat!(
                            "\r\n[Wardian] OpenCode exited without producing any output.\r\n",
                            "Possible causes:\r\n",
                            "  - generated OpenCode runtime config is invalid (check ~/.wardian/wardian_debug.log)\r\n",
                            "  - OpenCode binary not found or failed to start\r\n",
                            "  - Authentication/config error in OpenCode\r\n",
                            "Check ~/.wardian/wardian_debug.log for the exact command and config used.\r\n",
                        );
                        if let Ok(mut h) = output_buffer_clone.lock() {
                            h.push_str(msg);
                        }
                        let _ = pty_emit_app.emit(
                            "agent-pty-output-ready",
                            serde_json::json!({ "session_id": sid_for_pty }),
                        );
                    }
                    break;
                }
                Ok(n) => {
                    if provider_name_for_pty == "opencode" && opencode_chunks_logged < 40 {
                        log_debug(&format!(
                            "[Wardian] OpenCode PTY chunk {} for session {}: {}",
                            opencode_chunks_logged + 1,
                            sid_for_pty,
                            debug_preview_bytes(&buf[0..n], 256)
                        ));
                        opencode_chunks_logged += 1;
                    }
                    had_pty_output = true;
                    if let Ok(mut watch_state) = watch_state_clone.lock() {
                        watch_state.push_output(&buf[0..n]);
                    }
                    log_terminal_trace_bytes(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        "OUT",
                        &buf[0..n],
                    );
                    let text = pty_decoder.decode_chunk(&buf[0..n]);
                    if let Ok(mut stamp) = last_output_at_clone.lock() {
                        *stamp = Some(std::time::SystemTime::now());
                    }

                    // Process stream events to capture Session ID / Status changes
                    // Use a simple line-based approach for stream-json events
                    for line in text.lines() {
                        if let Some(event) = pty_provider.parse_output(line) {
                            match event {
                                AgentEvent::Init {
                                    session_id,
                                    timestamp,
                                } => {
                                    if let Some(ts) = timestamp {
                                        let mut it = init_timestamp_clone.lock().unwrap();
                                        if it.is_none() {
                                            *it = Some(ts);
                                        }
                                    }
                                    if !session_id.trim().is_empty() {
                                        let needs_save = {
                                            let mut config = config_lock_clone.lock().unwrap();
                                            if capture_codex_init_resume_session(
                                                &provider_name_for_pty,
                                                &session_id,
                                                &mut config,
                                            ) {
                                                true
                                            } else if provider_name_for_pty != "codex"
                                                && config.resume_session.as_deref()
                                                    != Some(&session_id)
                                            {
                                                config.resume_session = Some(session_id.clone());
                                                true
                                            } else {
                                                false
                                            }
                                        };
                                        if needs_save {
                                            log_debug(&format!(
                                                "[Wardian] Session ID mapped for {}: {}",
                                                sid_for_pty, session_id
                                            ));

                                            // Notify UI that metadata (resume_session ID) has changed
                                            let _ = pty_emit_app.emit("agents-updated", ());
                                            let _ = pty_emit_app.emit(
                                                "agent-pty-output-ready",
                                                serde_json::json!({ "session_id": sid_for_pty }),
                                            );
                                            save_agent_state_after_session_capture(&pty_emit_app);
                                        }
                                    }
                                }
                                AgentEvent::ActionRequired { message: _ } => {
                                    set_agent_status(
                                        &pty_app,
                                        &sid_for_pty,
                                        &current_status_clone,
                                        "Action Needed",
                                    );
                                }
                                AgentEvent::ModelResponse => {
                                    set_agent_status(
                                        &pty_app,
                                        &sid_for_pty,
                                        &current_status_clone,
                                        "Idle",
                                    );
                                }
                                AgentEvent::TurnCompleted => {
                                    set_agent_status(
                                        &pty_app,
                                        &sid_for_pty,
                                        &current_status_clone,
                                        "Idle",
                                    );
                                }
                                AgentEvent::UserQuery | AgentEvent::Generating => {
                                    set_agent_status(
                                        &pty_app,
                                        &sid_for_pty,
                                        &current_status_clone,
                                        "Processing...",
                                    );
                                }
                                _ => {}
                            }
                        }
                    }

                    if let Some(title) = extract_terminal_titles(&text).into_iter().last() {
                        let _previous_title = terminal_title_clone
                            .lock()
                            .map(|value| value.clone())
                            .unwrap_or_default();
                        if provider_name_for_pty == "opencode" {
                            log_debug(&format!(
                                "[Wardian] OpenCode backend title for session {}: {}",
                                sid_for_pty, title
                            ));
                        }
                        if let Ok(mut current_title) = terminal_title_clone.lock() {
                            *current_title = title.clone();
                        }
                        if provider_name_for_pty == "opencode" {
                            if let Some(next_status) = opencode_status_from_title(&title) {
                                set_agent_status(
                                    &pty_emit_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        } else if provider_name_for_pty == "gemini" {
                            if let Some(next_status) = gemini_status_from_title(&title) {
                                set_agent_status(
                                    &pty_emit_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        }
                    }
                    let should_emit_output_ready = if let Ok(mut h) = output_buffer_clone.lock() {
                        let was_empty = h.is_empty();
                        h.push_str(&text);
                        was_empty
                    } else {
                        false
                    };
                    if should_emit_output_ready {
                        let _ = pty_emit_app.emit(
                            "agent-pty-output-ready",
                            serde_json::json!({ "session_id": sid_for_pty }),
                        );
                    }
                    current_line.push_str(&text);
                    loop {
                        if let Some(start) = current_line.find('{') {
                            let slice = &current_line[start..];
                            let mut stream = serde_json::Deserializer::from_str(slice)
                                .into_iter::<serde_json::Value>();
                            match stream.next() {
                                Some(Ok(parsed)) => {
                                    // Use provider to classify the raw JSON into an AgentEvent
                                    let raw_line = parsed.to_string();
                                    if let Some(event) = pty_provider.parse_output(&raw_line) {
                                        if let AgentEvent::Init {
                                            ref session_id,
                                            ref timestamp,
                                        } = event
                                        {
                                            if let Some(ts) = timestamp {
                                                let mut it = init_timestamp_clone.lock().unwrap();
                                                if it.is_none() {
                                                    *it = Some(ts.clone());
                                                }
                                            }
                                            if !session_id.trim().is_empty() {
                                                let needs_save = {
                                                    let mut config =
                                                        config_lock_clone.lock().unwrap();
                                                    if capture_codex_init_resume_session(
                                                        &provider_name_for_pty,
                                                        session_id,
                                                        &mut config,
                                                    ) {
                                                        true
                                                    } else if provider_name_for_pty != "codex"
                                                        && config.resume_session.as_deref()
                                                            != Some(session_id.as_str())
                                                    {
                                                        config.resume_session =
                                                            Some(session_id.clone());
                                                        true
                                                    } else {
                                                        false
                                                    }
                                                };

                                                if needs_save {
                                                    log_debug(&format!(
                                                        "[Wardian] Session ID mapped for {}: {}",
                                                        sid_for_pty, session_id
                                                    ));
                                                    // Notify UI that metadata (resume_session ID) has changed
                                                    let _ = pty_emit_app.emit("agents-updated", ());
                                                    let _ = pty_emit_app.emit(
                                                        "agent-pty-output-ready",
                                                        serde_json::json!({ "session_id": sid_for_pty }),
                                                    );
                                                    save_agent_state_after_session_capture(
                                                        &pty_emit_app,
                                                    );
                                                }
                                            }
                                        }

                                        // Claude uses a dedicated log watcher for status, so
                                        // only capture Init timestamps from its PTY JSON.
                                        if provider_name_for_pty != "claude" {
                                            apply_agent_event(
                                                &pty_app,
                                                &sid_for_pty,
                                                event,
                                                &query_count_clone,
                                                &init_timestamp_clone,
                                                &current_status_clone,
                                            );
                                        }
                                    }
                                    let _ = pty_emit_app.emit("agent-json-event", serde_json::json!({ "session_id": sid_out, "data": parsed }));
                                    let consumed = stream.byte_offset();
                                    current_line = current_line[start + consumed..].to_string();
                                    continue;
                                }
                                _ => break,
                            }
                        }
                        break;
                    }
                    if current_line.len() > 10000 {
                        current_line.clear();
                    }
                }
                Err(err) => {
                    log_terminal_trace_note(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        &format!("pty read error: {}", err),
                    );
                    break;
                }
            }
        }
        // Process terminated (EOF or error) — mark status as Off
        set_agent_status(&pty_app, &sid_for_pty, &current_status_clone, "Off");
    });

    if config.provider == "codex" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_config = config_lock.clone();
        let watcher_skip_existing_log = is_restored;
        let wardian_agent_dir = get_wardian_home()
            .map(|home| home.join("agents").join(&watcher_session))
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().to_string());

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut last_lookup_session = String::new();
            let mut positioned_initial_log = !watcher_skip_existing_log;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }

                let path = {
                    let latest_session = latest_codex_session_index_entry(&watcher_session)
                        .ok()
                        .flatten()
                        .map(|(session_id, _updated_at)| session_id);
                    let lookup_session = watcher_config.lock().ok().and_then(|mut cfg| {
                        let previous_resume = cfg.resume_session.clone();
                        let previous_cleared = cfg.codex_cleared_provider_sessions.clone();
                        let lookup = codex_status_log_session(&mut cfg, latest_session);
                        if cfg.resume_session != previous_resume
                            || cfg.codex_cleared_provider_sessions != previous_cleared
                        {
                            let _ = watcher_app.emit("agents-updated", ());
                        }
                        lookup
                    });
                    let mut lock = watcher_log_path.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(lookup_session) = lookup_session {
                        if last_lookup_session != lookup_session {
                            *lock = None;
                            offset = 0;
                            positioned_initial_log = !watcher_skip_existing_log;
                            last_lookup_session = lookup_session.clone();
                        }
                        if lock.is_none() {
                            *lock = codex_session_file_path(
                                &lookup_session,
                                wardian_agent_dir.as_deref(),
                            );
                        }
                        lock.clone()
                    } else {
                        *lock = None;
                        offset = 0;
                        last_lookup_session.clear();
                        None
                    }
                };

                if let Some(path) = path {
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    let raw_line = parsed.to_string();
                                    if let Some(event) = watcher_provider.parse_output(&raw_line) {
                                        apply_agent_event(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_query_count,
                                            &watcher_init_timestamp,
                                            &watcher_current_status,
                                        );
                                    }
                                    let _ = watcher_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({ "session_id": watcher_session, "data": parsed }),
                                    );
                                }
                            }
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    } else if config.provider == "claude" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_folder = expected_folder.clone();
        let watcher_skip_existing_log = is_restored;
        let hook_event_log = claude_hook.as_ref().map(|hook| hook.event_log_path.clone());
        let waiting_for_permission = std::sync::Arc::new(std::sync::Mutex::new(false));
        let log_waiting_for_permission = waiting_for_permission.clone();

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut positioned_initial_log = !watcher_skip_existing_log;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }

                let path = {
                    let mut lock = watcher_log_path.lock().unwrap_or_else(|e| e.into_inner());
                    if lock.is_none() {
                        if let Some(home) = dirs::home_dir() {
                            let candidate = home
                                .join(".claude")
                                .join("projects")
                                .join(claude_project_dir_name(&watcher_folder))
                                .join(format!("{}.jsonl", watcher_session));
                            if candidate.exists() {
                                *lock = Some(candidate);
                            }
                        }
                    }
                    lock.clone()
                };

                if let Some(path) = path {
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                                positioned_initial_log = true;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                if let Some(event) = watcher_provider.parse_output(line.trim()) {
                                    let mut waiting = log_waiting_for_permission
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    if *waiting {
                                        match event {
                                            AgentEvent::UserQuery | AgentEvent::Generating => {
                                                if let Ok(parsed) =
                                                    serde_json::from_str::<serde_json::Value>(
                                                        line.trim(),
                                                    )
                                                {
                                                    let is_tool_result =
                                                        parsed.get("type").and_then(|v| v.as_str())
                                                            == Some("user")
                                                            && classify_claude_user_event(&parsed)
                                                                == ClaudeUserEventKind::ToolResult;
                                                    if is_tool_result {
                                                        *waiting = false;
                                                        apply_agent_status_event(
                                                            &watcher_app,
                                                            &watcher_session,
                                                            event,
                                                            &watcher_current_status,
                                                        );
                                                    }
                                                }
                                            }
                                            AgentEvent::ModelResponse => {
                                                *waiting = false;
                                                apply_agent_status_event(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                );
                                            }
                                            AgentEvent::ActionRequired { .. } => {
                                                apply_agent_status_event(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                );
                                            }
                                            AgentEvent::TurnCompleted => {
                                                *waiting = false;
                                                apply_agent_status_event(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                );
                                            }
                                            AgentEvent::Init { .. } | AgentEvent::Unknown => {}
                                        }
                                    } else {
                                        apply_agent_status_event(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_current_status,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });

        if let Some(hook_event_log) = hook_event_log {
            let hook_app = app.clone();
            let hook_session = config.session_id.clone();
            let hook_accepted_sessions = {
                let mut sessions = vec![config.session_id.clone()];
                if let Some(resume_session) = config
                    .resume_session
                    .as_ref()
                    .map(|sid| sid.trim())
                    .filter(|sid| !sid.is_empty() && *sid != config.session_id)
                {
                    sessions.push(resume_session.to_string());
                }
                if let Some(fresh_provider_session_id) = config
                    .fresh_provider_session_id
                    .as_ref()
                    .map(|sid| sid.trim())
                    .filter(|sid| !sid.is_empty() && *sid != config.session_id)
                {
                    sessions.push(fresh_provider_session_id.to_string());
                }
                sessions
            };
            let hook_current_status = current_status.clone();
            let hook_waiting_for_permission = waiting_for_permission.clone();

            std::thread::spawn(move || {
                let mut offset = 0;
                loop {
                    let current = hook_current_status
                        .lock()
                        .map(|s| s.clone())
                        .unwrap_or_else(|e| e.into_inner().clone());
                    if current == "Off" {
                        break;
                    }

                    if let Ok(mut file) = std::fs::File::open(&hook_event_log) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    if !hook_accepted_sessions.iter().any(|session_id| {
                                        claude_permission_hook_matches_session(&parsed, session_id)
                                    }) {
                                        continue;
                                    }
                                    if let Ok(mut waiting) = hook_waiting_for_permission.lock() {
                                        *waiting = true;
                                    }
                                    let tool_name = parsed
                                        .get("tool_name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Tool approval required")
                                        .to_string();
                                    apply_agent_status_event(
                                        &hook_app,
                                        &hook_session,
                                        AgentEvent::ActionRequired {
                                            message: tool_name.clone(),
                                        },
                                        &hook_current_status,
                                    );
                                    let _ = hook_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({
                                            "session_id": hook_session,
                                            "data": {
                                                "type": "system",
                                                "subtype": "permission_request",
                                                "tool_name": tool_name,
                                            }
                                        }),
                                    );
                                }
                            }
                        }
                    }

                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            });
        }
    }

    // ── OpenCode log-file watcher ─────────────────────────────────────────
    {
        let mut cfg = config_lock.lock().unwrap();
        cfg.folder = expected_folder;
    }

    Ok(ActiveAgent {
        config: config_lock,
        child_process: Some(child),
        background_processes,
        pty_master: Some(pty_master),
        stdin_tx: Some(tx),
        output_buffer,
        process_id,
        query_count,
        init_timestamp,
        current_status,
        last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
        watch_state,
        terminal_title,
        last_output_at,
        log_path,
        log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
        #[cfg(windows)]
        job_object,
    })
}

pub async fn resize_pty(
    session_id: String,
    cols: u16,
    rows: u16,
    state: &AppState,
) -> Result<(), String> {
    if cols < 10 {
        return Ok(());
    }
    let master_arc = {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&session_id) {
            #[cfg(feature = "terminal-trace")]
            {
                let provider = agent
                    .config
                    .lock()
                    .map(|config| config.provider.clone())
                    .unwrap_or_else(|poisoned| poisoned.into_inner().provider.clone());
                log_terminal_trace_note(
                    &session_id,
                    &provider,
                    &format!("resize cols={} rows={}", cols, rows),
                );
            }
            agent
                .pty_master
                .clone()
                .ok_or_else(|| format!("Agent {} is off", session_id))?
        } else {
            return Err(format!("Agent {} not found", session_id));
        }
    };
    tokio::task::spawn_blocking(move || {
        let master = match master_arc.lock() {
            Ok(master) => master,
            Err(poisoned) => poisoned.into_inner(),
        };
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    })
    .await
    .map_err(|e| format!("Failed to join PTY resize task: {}", e))?
    .map_err(|e| format!("Failed to resize PTY: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_status_log_session_tracks_excluded_latest_without_adopting_resume() {
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            resume_session: None,
            codex_cleared_provider_sessions: vec!["provider-session-1".to_string()],
            ..Default::default()
        };

        let log_session =
            codex_status_log_session(&mut config, Some("provider-session-1".to_string()));

        assert_eq!(log_session.as_deref(), Some("provider-session-1"));
        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.codex_cleared_provider_sessions,
            vec!["provider-session-1".to_string()]
        );
    }
}
