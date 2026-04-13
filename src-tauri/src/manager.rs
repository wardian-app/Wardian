use crate::models::{AgentClassDefinition, AgentConfig, AgentEvent, AgentTelemetry};
use crate::providers::claude::{classify_claude_user_event, ClaudeUserEventKind};
use crate::providers::opencode::OpenCodeProvider;
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AppState};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{BufRead, Read, Seek, Write};
use tauri::{AppHandle, Emitter, Manager};

pub use crate::utils::fs::*;
pub use crate::utils::logging::{log_debug, log_terminal_trace_bytes, log_terminal_trace_note};
pub use crate::utils::process::new_headless_command;
#[cfg(windows)]
pub use crate::utils::process::{find_wardian_session_process_roots, force_kill_process_tree};
pub use crate::utils::shell::build_program_launch;

#[cfg(windows)]
fn cleanup_stale_session_processes(session_id: &str, provider: &str) {
    for pid in find_wardian_session_process_roots(session_id, Some(std::process::id())) {
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

pub fn terminate_active_agent_process(agent: &mut ActiveAgent) {
    // IMPORTANT: Kill the process tree FIRST while the parent is still alive.
    // If we kill the PTY child (cmd.exe) first, its children (claude.exe, node.exe,
    // etc.) become orphaned and taskkill /T can no longer enumerate them via parent PID.
    #[cfg(windows)]
    {
        if let Some(pid) = agent.process_id {
            if let Err(err) = force_kill_process_tree(pid) {
                log_debug(&format!(
                    "[Wardian] Failed to force-kill process tree for session {} via PID {}: {}",
                    agent.config.session_id, pid, err
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
                log_debug(&format!(
                    "[Wardian] Failed to force-kill background process for session {} via PID {}: {}",
                    agent.config.session_id,
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

fn set_agent_status(
    app: &AppHandle,
    session_id: &str,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
    next_status: &str,
) {
    if let Ok(mut status) = current_status.lock() {
        if *status != next_status {
            *status = next_status.to_string();
            let _ = app.emit(
                "agent-status-updated",
                serde_json::json!({
                    "session_id": session_id,
                    "current_status": next_status,
                }),
            );
        }
    }
}

fn opencode_status_from_title(title: &str) -> Option<&'static str> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "OpenCode" {
        return Some("Idle");
    }
    if trimmed.contains("Action Required") {
        return Some("Action Needed");
    }
    if trimmed.starts_with("OC | ") {
        return Some("Processing...");
    }
    None
}

fn opencode_session_diff_path(session_id: &str) -> std::path::PathBuf {
    let base = dirs::data_local_dir().or_else(|| {
        dirs::home_dir().map(|home| home.join(".local").join("share"))
    });
    let base = base.unwrap_or_else(|| std::path::PathBuf::from("."));
    base
        .join("opencode")
        .join("storage")
        .join("session_diff")
        .join(format!("{session_id}.json"))
}

fn opencode_should_fallback_to_idle(
    current_status: &str,
    last_output_at: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> bool {
    if current_status != "Processing..." {
        return false;
    }
    let Some(last_output_at) = last_output_at else {
        return false;
    };
    now.duration_since(last_output_at)
        .map(|duration| duration >= std::time::Duration::from_secs(6))
        .unwrap_or(false)
}

fn debug_preview_bytes(bytes: &[u8], limit: usize) -> String {
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

fn extract_terminal_titles(chunk: &str) -> Vec<String> {
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

            let title = String::from_utf8_lossy(&bytes[value_start..end]).trim().to_string();
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

fn apply_agent_event(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    query_count: &std::sync::Arc<std::sync::Mutex<usize>>,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) {
    match event {
        AgentEvent::UserQuery => {
            if let Ok(mut count) = query_count.lock() {
                *count += 1;
            }
            set_agent_status(app, session_id, current_status, "Processing...");
        }
        AgentEvent::Generating => {
            set_agent_status(app, session_id, current_status, "Processing...");
        }
        AgentEvent::Init { timestamp, .. } => {
            if let Ok(mut ts) = init_timestamp.lock() {
                *ts = timestamp;
            }
        }
        AgentEvent::ModelResponse => {
            set_agent_status(app, session_id, current_status, "Idle");
        }
        AgentEvent::TurnCompleted => {
            set_agent_status(app, session_id, current_status, "Idle");
        }
        AgentEvent::ActionRequired { .. } => {
            set_agent_status(app, session_id, current_status, "Action Needed");
        }
        AgentEvent::Unknown => {}
    }
}

fn apply_agent_status_event(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) {
    match event {
        AgentEvent::UserQuery | AgentEvent::Generating => {
            set_agent_status(app, session_id, current_status, "Processing...");
        }
        AgentEvent::ModelResponse => {
            set_agent_status(app, session_id, current_status, "Idle");
        }
        AgentEvent::TurnCompleted => {
            set_agent_status(app, session_id, current_status, "Idle");
        }
        AgentEvent::ActionRequired { .. } => {
            set_agent_status(app, session_id, current_status, "Action Needed");
        }
        AgentEvent::Init { .. } | AgentEvent::Unknown => {}
    }
}

/// On macOS, GUI apps inherit a minimal PATH that excludes Homebrew, npm globals,
/// Volta, and other user-level tool installs. Prepend the common locations so that
/// `claude`, `gemini`, and similar CLIs can be found when spawning child processes.
#[cfg(target_os = "macos")]
fn macos_extended_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let existing = std::env::var("PATH").unwrap_or_default();
    let extra = format!(
        "{home}/.local/bin:/usr/local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:{home}/.npm-global/bin:{home}/.volta/bin",
        home = home
    );
    if existing.is_empty() {
        format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", extra)
    } else {
        format!("{}:{}", extra, existing)
    }
}

pub fn save_state(_app: &AppHandle, agents: &HashMap<String, ActiveAgent>, order: &[String]) {
    let mut configs: Vec<AgentConfig> = Vec::new();
    for id in order {
        if let Some(agent) = agents.get(id) {
            configs.push(agent.config.clone());
        }
    }

    if let Ok(json) = serde_json::to_string_pretty(&configs) {
        if let Some(app_dir) = get_wardian_home() {
            let _ = std::fs::create_dir_all(&app_dir);
            let state_path = app_dir.join("wardian_state.json");
            let _ = std::fs::write(state_path, json);
        }
    }
}

pub async fn spawn_agent(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
) -> Result<ActiveAgent, String> {
    let provider = ProviderFactory::resolve(&config.provider)?;

    let cwd = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);

    let expected_folder = if config.folder.is_empty() {
        cwd.to_string_lossy().to_string()
    } else {
        config.folder.clone()
    };

    if config.is_off {
        return Ok(ActiveAgent {
            config,
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Off".to_string())),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        });
    }

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
    log_terminal_trace_note(
        &config.session_id,
        &config.provider,
        &format!(
            "spawn cwd={} restored={} args={:?}",
            provider_cwd.display(),
            is_restored,
            provider_args
        ),
    );

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    let process_id = child.process_id();

    #[cfg(windows)]
    let job_object = {
        if let Ok(job) = win32job::Job::create() {
            let mut info = job.query_extended_limit_info().unwrap_or_default();
            info.limit_kill_on_job_close();
            let _ = job.set_extended_limit_info(&info);
            if let Some(pid) = process_id {
                unsafe {
                    use winapi::um::processthreadsapi::OpenProcess;
                    use winapi::um::winnt::{PROCESS_SET_QUOTA, PROCESS_TERMINATE};
                    let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                    if !handle.is_null() {
                        let _ = job.assign_process(handle as isize);
                        winapi::um::handleapi::CloseHandle(handle);
                    }
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
    let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(Some(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )));
    let init_timestamp_clone = init_timestamp.clone();
    let current_status = std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string()));
    let current_status_clone = current_status.clone();
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
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut current_line = String::new();
        let mut had_pty_output = false;
        let mut opencode_chunks_logged = 0usize;
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
                    log_terminal_trace_bytes(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        "OUT",
                        &buf[0..n],
                    );
                    let text = String::from_utf8_lossy(&buf[0..n]).to_string();
                    if let Ok(mut stamp) = last_output_at_clone.lock() {
                        *stamp = Some(std::time::SystemTime::now());
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
                                        // Claude uses a dedicated log watcher for status, so
                                        // only capture Init timestamps from its PTY JSON.
                                        if provider_name_for_pty == "claude" {
                                            if let AgentEvent::Init { timestamp, .. } = event {
                                                if let Ok(mut ts) = init_timestamp_clone.lock() {
                                                    *ts = timestamp;
                                                }
                                            }
                                        } else {
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
        let wardian_agent_dir = get_wardian_home()
            .map(|home| home.join("agents").join(&watcher_session))
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().to_string());

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
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
                        *lock =
                            codex_session_file_path(&watcher_session, wardian_agent_dir.as_deref());
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
        let hook_event_log = claude_hook.as_ref().map(|hook| hook.event_log_path.clone());
        let waiting_for_permission = std::sync::Arc::new(std::sync::Mutex::new(false));
        let log_waiting_for_permission = waiting_for_permission.clone();

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
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
    Ok(ActiveAgent {
        config: AgentConfig {
            folder: expected_folder,
            ..config
        },
        child_process: Some(child),
        background_processes,
        pty_master: Some(pty_master),
        stdin_tx: Some(tx),
        output_buffer,
        process_id,
        query_count,
        init_timestamp,
        current_status,
        terminal_title,
        last_output_at,
        log_path,
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
            log_terminal_trace_note(
                &session_id,
                &agent.config.provider,
                &format!("resize cols={} rows={}", cols, rows),
            );
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

fn codex_bootstrap_workspace_key(workspace_cwd: &std::path::Path) -> String {
    let normalized = workspace_cwd.to_string_lossy().to_ascii_lowercase();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("workspace-{hash:016x}")
}

fn strip_flag_value_pairs(args: Vec<String>, flag: &str) -> Vec<String> {
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

fn strip_standalone_flag(args: Vec<String>, flag: &str) -> Vec<String> {
    args.into_iter().filter(|arg| arg != flag).collect()
}

fn persisted_agent_config(session_id: &str) -> Option<AgentConfig> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }

    let wardian_home = get_wardian_home()?;
    let state_path = wardian_home.join("wardian_state.json");
    let contents = std::fs::read_to_string(state_path).ok()?;
    let configs = serde_json::from_str::<Vec<AgentConfig>>(&contents).ok()?;
    configs
        .into_iter()
        .find(|config| config.session_id == session_id)
}

fn opencode_runtime_config_content(
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Option<String> {
    let roots = resolve_opencode_runtime_roots(
        class_name,
        session_id,
        config.and_then(|cfg| cfg.system_include_directories.as_deref()),
        config.and_then(|cfg| cfg.include_directories.as_deref()),
    );
    let runtime_config = build_opencode_runtime_config(&roots);
    runtime_config
        .as_object()
        .filter(|map| !map.is_empty())
        .map(|_| runtime_config.to_string())
}

fn opencode_runtime_roots(
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Vec<std::path::PathBuf> {
    resolve_opencode_runtime_roots(
        class_name,
        session_id,
        config.and_then(|cfg| cfg.system_include_directories.as_deref()),
        config.and_then(|cfg| cfg.include_directories.as_deref()),
    )
}

fn opencode_custom_config_dir(
    cwd: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<Option<std::path::PathBuf>, String> {
    let roots = opencode_runtime_roots(class_name, session_id, config);
    if roots.is_empty() {
        return Ok(None);
    }

    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let key = codex_bootstrap_workspace_key(cwd);
    let config_dir =
        if let Some(session_id) = session_id.map(str::trim).filter(|sid| !sid.is_empty()) {
            wardian_home
                .join("agents")
                .join(session_id)
                .join("habitat")
                .join(".opencode")
        } else {
            wardian_home
                .join("provider-bootstrap")
                .join("opencode")
                .join(key)
                .join(".opencode")
        };

    // Sync the custom config dir and create the merged skills tree.
    crate::utils::fs::sync_opencode_config_dir(&config_dir, &roots)?;
    Ok(Some(config_dir))
}

fn opencode_env(
    cwd: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<Vec<(String, String)>, String> {
    let mut envs = vec![("COLORTERM".to_string(), "truecolor".to_string())];
    if let Some(tui_config) = crate::utils::get_opencode_tui_path() {
        envs.push((
            "OPENCODE_TUI_CONFIG".to_string(),
            tui_config.to_string_lossy().to_string(),
        ));
    }
    if let Some(config_dir) = opencode_custom_config_dir(cwd, class_name, session_id, config)? {
        let config_path = config_dir.join("opencode.json");

        // Build the runtime config (instructions + theme), and pair it with a
        // custom config directory so OpenCode can discover projected skills.
        let runtime_config: serde_json::Value =
            opencode_runtime_config_content(class_name, session_id, config)
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({"theme": "system"}));

        std::fs::write(&config_path, runtime_config.to_string()).map_err(|e| e.to_string())?;
        envs.push((
            "OPENCODE_CONFIG_DIR".to_string(),
            config_dir.to_string_lossy().to_string(),
        ));
        envs.push((
            "OPENCODE_CONFIG".to_string(),
            config_path.to_string_lossy().to_string(),
        ));
    }
    Ok(envs)
}

fn opencode_interactive_env(
    cwd: &std::path::Path,
    config: &AgentConfig,
) -> Result<Vec<(String, String)>, String> {
    opencode_env(
        cwd,
        &config.agent_class,
        Some(config.session_id.as_str()),
        Some(config),
    )
}

fn codex_bootstrap_launch_context(
    wardian_home: &std::path::Path,
    workspace_cwd: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let bootstrap_home = wardian_home
        .join("provider-bootstrap")
        .join("codex")
        .join(codex_bootstrap_workspace_key(workspace_cwd))
        .join(".codex");
    (workspace_cwd.to_path_buf(), bootstrap_home)
}

fn interactive_provider_cwd(
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

fn interactive_provider_args(
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
            let target_dir = if provider_cwd.file_name().is_some_and(|name| name == "habitat") {
                habitat_workspace_cwd(provider_cwd)
            } else {
                workspace_cwd.to_path_buf()
            };
            provider_args.push(target_dir.to_string_lossy().replace('\\', "/"));
        }
        _ => {}
    }

    provider_args
}

fn finalize_interactive_spawn_args(
    provider_name: &str,
    _is_restored: bool,
    _resume_session: &Option<String>,
    mut provider_args: Vec<String>,
) -> Vec<String> {
    if provider_name == "claude" {
        provider_args = strip_standalone_flag(provider_args, "--verbose");
        provider_args = strip_flag_value_pairs(provider_args, "--input-format");
        provider_args = strip_flag_value_pairs(provider_args, "--output-format");
    }
    provider_args
}

fn headless_provider_launch(
    provider_name: &str,
    bin: &str,
    provider_args: &[String],
) -> Result<crate::utils::shell::ShellLaunchSpec, String> {
    #[cfg(windows)]
    if provider_name == "opencode" {
        let cmd_host = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut fragments = vec![quote_cmd_arg(bin)];
        fragments.extend(provider_args.iter().map(|arg| quote_cmd_arg(arg)));
        return Ok(crate::utils::shell::ShellLaunchSpec {
            executable: cmd_host,
            args: vec!["/d".to_string(), "/c".to_string(), fragments.join(" ")],
        });
    }

    build_program_launch(bin, provider_args)
}

fn interactive_provider_launch(
    provider_name: &str,
    bin: &str,
    provider_args: &[String],
) -> Result<crate::utils::shell::ShellLaunchSpec, String> {
    let _ = provider_name;
    Ok(crate::utils::shell::ShellLaunchSpec {
        executable: bin.to_string(),
        args: provider_args.to_vec(),
    })
}

fn apply_terminal_identity_env(cmd: &mut CommandBuilder) {
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM", "xterm-256color");
}


#[cfg(windows)]
fn quote_cmd_arg(value: &str) -> String {
    let escaped = value.replace('"', r#"\""#);
    if escaped.is_empty()
        || escaped
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '^' | '&' | '|' | '<' | '>' | '(' | ')'))
    {
        format!("\"{}\"", escaped)
    } else {
        escaped
    }
}

fn migrate_codex_bootstrap_home(
    bootstrap_home: &std::path::Path,
    final_home: &std::path::Path,
) -> Result<(), String> {
    if !bootstrap_home.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(final_home).map_err(|e| e.to_string())?;

    let entries = std::fs::read_dir(bootstrap_home).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let source = entry.path();
        let name = entry.file_name();
        let target = final_home.join(&name);
        let name_str = name.to_string_lossy();

        if matches!(name_str.as_ref(), "auth.json" | "config.toml" | "cap_sid") {
            continue;
        }

        if name_str == "skills" {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            let skill_entries = std::fs::read_dir(&source).map_err(|e| e.to_string())?;
            for skill_entry in skill_entries.flatten() {
                let skill_source = skill_entry.path();
                let skill_target = target.join(skill_entry.file_name());
                if skill_target.exists() || skill_target.symlink_metadata().is_ok() {
                    continue;
                }
                std::fs::rename(&skill_source, &skill_target).map_err(|e| e.to_string())?;
            }
            let _ = std::fs::remove_dir_all(&source);
            continue;
        }

        if target.exists() || target.symlink_metadata().is_ok() {
            continue;
        }

        std::fs::rename(&source, &target).map_err(|e| e.to_string())?;
    }

    std::fs::create_dir_all(bootstrap_home).map_err(|e| e.to_string())?;

    Ok(())
}

pub async fn run_headless(
    cwd: &std::path::Path,
    prompt: &str,
    session_id: &str,
    output_format: &str,
    provider_name: &str,
) -> Result<serde_json::Value, String> {
    run_headless_with_config(cwd, prompt, session_id, output_format, provider_name, None).await
}

pub async fn run_headless_with_config(
    cwd: &std::path::Path,
    prompt: &str,
    session_id: &str,
    output_format: &str,
    provider_name: &str,
    config_override: Option<&AgentConfig>,
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let provider = ProviderFactory::resolve(provider_name)?;
    let habitat_root = prepare_provider_habitat(provider_name, cwd, "", Some(session_id))?;
    let provider_cwd = cwd.to_path_buf();
    let normalized_prompt = if provider_name == "opencode" {
        crate::utils::terminal_input::normalize_prompt_for_terminal_submit(prompt)
    } else {
        prompt.to_string()
    };
    let persisted_opencode_config = if provider_name == "opencode" {
        config_override.cloned().or_else(|| persisted_agent_config(session_id))
    } else {
        None
    };
    let (bin, mut provider_args) = provider.get_executable();
    let claude_hook = if provider_name == "claude" {
        ensure_claude_permission_hook(session_id).ok()
    } else {
        None
    };
    match provider_name {
        "codex" => {
            provider_args.push("--cd".to_string());
            provider_args.push(provider_cwd.to_string_lossy().to_string());
            provider_args.push("exec".to_string());
            provider_args.push("resume".to_string());
            provider_args.push(session_id.to_string());
            provider_args.push("--json".to_string());
            provider_args.push(prompt.to_string());
        }
        "claude" => {
            if let Some(hook) = claude_hook.as_ref() {
                provider_args.push("--settings".to_string());
                provider_args.push(hook.settings_arg.clone());
            }
            provider_args.push("--print".to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push(output_format.to_string());
            if !session_id.is_empty() {
                provider_args.push("--resume".to_string());
                provider_args.push(session_id.to_string());
            }
            provider_args.push(prompt.to_string());
        }
        "mock" => {
            provider_args.push("--print".to_string());
            provider_args.push(prompt.to_string());
        }
        "opencode" => {
            provider_args.push("run".to_string());
            if let Some(config) = persisted_opencode_config.as_ref() {
                provider_args.extend(provider.get_spawn_args(config, !session_id.is_empty()));
            } else if !session_id.is_empty() {
                provider_args.push("--session".to_string());
                provider_args.push(session_id.to_string());
            }
            provider_args.push("--format".to_string());
            provider_args.push("json".to_string());
            provider_args.push("--dir".to_string());
            provider_args.push(provider_cwd.to_string_lossy().to_string());
            provider_args.push(normalized_prompt.clone());
        }
        _ => {
            provider_args.push("-p".to_string());
            provider_args.push(prompt.to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push(output_format.to_string());
            if !session_id.is_empty() {
                provider_args.push("--resume".to_string());
                provider_args.push(session_id.to_string());
            }
        }
    }

    let launch_spec = headless_provider_launch(provider_name, &bin, &provider_args)?;
    let mut cmd = new_headless_command(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }
    if provider_name == "codex" {
        if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
    } else if provider_name == "opencode" {
        let class_name = persisted_opencode_config
            .as_ref()
            .map(|config| config.agent_class.as_str())
            .unwrap_or("");
        for (key, value) in opencode_env(
            &provider_cwd,
            class_name,
            (!session_id.is_empty()).then_some(session_id),
            persisted_opencode_config.as_ref(),
        )? {
            cmd.env(key, value);
        }
        cmd.stdin(std::process::Stdio::null());
    } else if provider_name == "mock" {
        if let Ok(scenario) = std::env::var("WARDIAN_MOCK_SCENARIO") {
            cmd.env("WARDIAN_MOCK_SCENARIO", scenario);
        }
        if let Ok(delay) = std::env::var("WARDIAN_MOCK_DELAY_MS") {
            cmd.env("WARDIAN_MOCK_DELAY_MS", delay);
        }
        if let Ok(script) = std::env::var("WARDIAN_MOCK_SCRIPT") {
            cmd.env("WARDIAN_MOCK_SCRIPT", script);
        }
    }
    cmd.current_dir(&provider_cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    log_debug(&format!(
        "[Wardian] run_headless: provider={}, session_id={}, cwd={}, prompt_len={}, output_format={}",
        provider_name,
        if session_id.is_empty() { "<none>" } else { session_id },
        cwd.display(),
        prompt.len(),
        output_format
    ));
    log_debug(&format!(
        "[Wardian] run_headless args: exe={} args={:?}",
        launch_spec.executable, launch_spec.args
    ));

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    // Read stdout and stderr concurrently to avoid deadlock when stderr buffer fills.
    let stdout_handle = {
        let stdout = child.stdout.take();
        tokio::spawn(async move {
            let mut out = String::new();
            if let Some(stream) = stdout {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    out.push_str(&line);
                    line.clear();
                }
            }
            out
        })
    };

    let stderr_handle = {
        let stderr = child.stderr.take();
        tokio::spawn(async move {
            let mut err = String::new();
            if let Some(stream) = stderr {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    err.push_str(&line);
                    line.clear();
                }
            }
            err
        })
    };

    let (output, err_output) = tokio::join!(stdout_handle, stderr_handle);
    let output = output.unwrap_or_default();
    let err_output = err_output.unwrap_or_default();

    let _ = child.wait().await;

    if !err_output.is_empty() {
        log_debug(&format!("[Wardian] Headless stderr: {}", err_output.trim()));
    }

    if provider_name == "codex" {
        let mut last_message = None;
        for line in output.lines() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
                match parsed.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "item.completed" => {
                        if parsed
                            .get("item")
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            == Some("agent_message")
                        {
                            last_message = parsed
                                .get("item")
                                .and_then(|v| v.get("text"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                    "event_msg" => {
                        if parsed
                            .get("payload")
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            == Some("agent_message")
                        {
                            last_message = parsed
                                .get("payload")
                                .and_then(|v| v.get("message"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        if output_format == "json" {
            Ok(serde_json::json!({
                "thread_id": session_id,
                "response": last_message.unwrap_or_default(),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": last_message.unwrap_or(output) }))
        }
    } else if provider_name == "opencode" {
        let summary = OpenCodeProvider::summarize_run_output(&output);

        if output_format == "json" {
            Ok(serde_json::json!({
                "session_id": summary.session_id.unwrap_or_else(|| session_id.to_string()),
                "response": summary.last_text.clone().unwrap_or_default(),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": summary.last_text.unwrap_or(output) }))
        }
    } else if output_format == "json" {
        serde_json::from_str(&output)
            .map_err(|e| format!("Failed to parse JSON output: {}. Raw: {}", e, output))
    } else {
        Ok(serde_json::json!({ "text": output }))
    }
}

pub async fn obtain_session_id(
    cwd: &std::path::Path,
    agent_class: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<String, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let provider_name = config.map(|c| c.provider.as_str()).unwrap_or("claude");
    let provider = ProviderFactory::resolve(provider_name)?;
    let (bin, mut provider_args) = provider.get_executable();
    let class_name = agent_class
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            config.and_then(|cfg| {
                (!cfg.agent_class.trim().is_empty()).then_some(cfg.agent_class.as_str())
            })
        })
        .unwrap_or("");
    let habitat_root = prepare_provider_habitat(provider_name, cwd, class_name, None)?;
    let codex_bootstrap = if provider_name == "codex" {
        let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
        Some(codex_bootstrap_launch_context(&wardian_home, cwd))
    } else {
        None
    };
    let provider_cwd = interactive_provider_cwd(
        provider_name,
        cwd,
        habitat_root.as_deref(),
        codex_bootstrap.as_ref(),
    );

    if provider_name == "codex" {
        provider_args.push("--cd".to_string());
        provider_args.push(provider_cwd.to_string_lossy().to_string());
        provider_args.push("exec".to_string());

        if let Some(config) = config {
            let spawn_args = strip_flag_value_pairs(provider.get_spawn_args(config, false), "--add-dir");
            provider_args.extend(strip_standalone_flag(spawn_args, "--no-alt-screen"));
            if config.codex_skip_git_repo_check.unwrap_or(true) {
                provider_args.push("--skip-git-repo-check".to_string());
            }
            if config.codex_ephemeral.unwrap_or(false) {
                provider_args.push("--ephemeral".to_string());
            }
        }

        provider_args.push("--json".to_string());
        provider_args.push("Introduce yourself".to_string());
    } else if provider_name == "opencode" {
        provider_args.push("run".to_string());
        if let Some(config) = config {
            provider_args.extend(provider.get_spawn_args(config, false));
        }
        provider_args.push("--format".to_string());
        provider_args.push("json".to_string());
        provider_args.push("--dir".to_string());
        provider_args.push(cwd.to_string_lossy().to_string());
        provider_args.push("Introduce yourself".to_string());
    } else if provider_name == "claude" {
        // --print mode does not accept --input-format stream-json; strip it.
        if let Some(config) = config {
            let spawn_args = strip_flag_value_pairs(
                provider.get_spawn_args(config, false),
                "--input-format",
            );
            provider_args.extend(spawn_args);
        } else {
            provider_args.push("--verbose".to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push("stream-json".to_string());
        }
        provider_args.push("--print".to_string());
        provider_args.push("Introduce yourself".to_string());
    } else {
        provider_args.push("-p".to_string());
        provider_args.push("Introduce yourself".to_string());
        provider_args.push("-o".to_string());
        provider_args.push("stream-json".to_string());
    }

    let launch_spec = headless_provider_launch(provider_name, &bin, &provider_args)?;
    let mut cmd = new_headless_command(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }

    if provider_name == "codex" {
        if let Some((_, bootstrap_home)) = codex_bootstrap.as_ref() {
            let real_codex_home = dirs::home_dir()
                .ok_or("Could not find user home directory")?
                .join(".codex");
            sync_codex_agent_home(&real_codex_home, bootstrap_home, std::path::Path::new(""))?;
            cmd.env("CODEX_HOME", bootstrap_home);
        } else if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if provider_name == "opencode" {
        for (key, value) in opencode_env(cwd, class_name, None, config)? {
            cmd.env(key, value);
        }
        cmd.stdin(std::process::Stdio::null());
    } else if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
        cmd.stdin(std::process::Stdio::null());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }

    let command_cwd = if provider_name == "claude" {
        cwd.to_path_buf()
    } else {
        provider_cwd.clone()
    };

    cmd.current_dir(&command_cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    log_debug(&format!(
        "[WARDIAN-DEBUG] Running obtain_session_id for provider {}",
        provider_name
    ));
    log_debug(&format!(
        "[WARDIAN-DEBUG] obtain_session_id launch: exe={} args={:?} cwd={}",
        launch_spec.executable,
        launch_spec.args,
        command_cwd.display()
    ));
    match cmd.spawn() {
        Ok(mut child) => {
            log_debug("[WARDIAN-DEBUG] Spawned headless process. Reading stdout...");
            let mut session_id_res = None;
            let mut stderr_output = String::new();

            let timeout = tokio::time::Duration::from_secs(60);
            let read_future = async {
                let mut session_id: Option<String> = None;
                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout);
                    let mut line = String::new();
                    while let Ok(n) = reader.read_line(&mut line).await {
                        if n == 0 {
                            log_debug("[WARDIAN-DEBUG] Reached EOF on stdout.");
                            break;
                        }
                        let trimmed = line.trim();
                        if let Some(start) = trimmed.find('{') {
                            let json_part = &trimmed[start..];
                            if provider_name == "opencode" {
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_part) {
                                    if session_id.is_none() {
                                        session_id = parsed
                                            .get("sessionID")
                                            .and_then(|value| value.as_str())
                                            .map(|value| value.to_string());
                                    }
                                }
                            }
                            if let Some(evt) = provider.parse_output(json_part) {
                                match evt {
                                    AgentEvent::Init {
                                        session_id: sid, ..
                                    } if !sid.is_empty() => {
                                        log_debug(&format!(
                                            "[WARDIAN-DEBUG] Found session_id: {}",
                                            sid
                                        ));
                                        session_id = Some(sid);
                                    }
                                    // ModelResponse means the prompt completed and the session
                                    // has been persisted to disk — safe to stop reading.
                                    AgentEvent::ModelResponse => {
                                        log_debug(
                                            "[WARDIAN-DEBUG] Prompt complete, session saved.",
                                        );
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        line.clear();
                    }
                }
                session_id
            };

            let timed_out = match tokio::time::timeout(timeout, read_future).await {
                Ok(sid) => {
                    session_id_res = sid;
                    false
                }
                Err(_) => {
                    log_debug("[WARDIAN-DEBUG] Timed out waiting for session_id.");
                    true
                }
            };

            // Only force-kill if we timed out; otherwise let the process exit naturally
            // so the session is fully flushed to disk before we attempt --resume.
            if timed_out {
                let _ = child.kill().await;
            }
            if let Some(stderr) = child.stderr.take() {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    stderr_output.push_str(&line);
                    line.clear();
                }
            }
            let _ = child.wait().await;
            if session_id_res.is_none() && !stderr_output.trim().is_empty() {
                log_debug(&format!(
                    "[WARDIAN-DEBUG] obtain_session_id stderr: {}",
                    stderr_output.trim()
                ));
            }
            if provider_name == "codex" {
                if let (Some(session_id), Some((_, bootstrap_home))) =
                    (session_id_res.as_ref(), codex_bootstrap.as_ref())
                {
                    if let Some(final_habitat_root) =
                        prepare_provider_habitat(provider_name, cwd, class_name, Some(session_id))?
                    {
                        migrate_codex_bootstrap_home(
                            bootstrap_home,
                            &habitat_codex_home(&final_habitat_root),
                        )?;
                    }
                }
            }
            log_debug(&format!(
                "[WARDIAN-DEBUG] Returning session_id: {:?}",
                session_id_res
            ));
            session_id_res.ok_or_else(|| {
                if stderr_output.trim().is_empty() {
                    format!(
                        "Provider {} did not return a session ID during initialization.",
                        provider_name
                    )
                } else {
                    stderr_output.trim().to_string()
                }
            })
        }
        Err(e) => {
            log_debug(&format!("[WARDIAN-DEBUG] Failed to spawn cmd: {:?}", e));
            Err(format!(
                "Failed to spawn {} bootstrap command: {}",
                provider_name, e
            ))
        }
    }
}

/// Converts a workspace absolute path into Claude Code's project directory name.
/// Claude replaces each of `:`, `\`, `/`, `.` with `-`.
/// e.g. `D:\Development\Wardian` → `D--Development-Wardian`
fn claude_project_dir_name(workspace: &str) -> String {
    workspace
        .chars()
        .map(|c| match c {
            ':' | '\\' | '/' | '.' => '-',
            _ => c,
        })
        .collect()
}

fn codex_session_file_path_in(
    base: &std::path::Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let base = base.join("sessions");
    let years = std::fs::read_dir(base).ok()?;

    for year in years.flatten() {
        let months = match std::fs::read_dir(year.path()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for month in months.flatten() {
            let days = match std::fs::read_dir(month.path()) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for day in days.flatten() {
                let files = match std::fs::read_dir(day.path()) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for file in files.flatten() {
                    let path = file.path();
                    if !path.is_file() {
                        continue;
                    }
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if file_name.ends_with(&format!("{}.jsonl", session_id)) {
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

fn codex_session_file_path(
    session_id: &str,
    wardian_agent_dir: Option<&str>,
) -> Option<std::path::PathBuf> {
    if let Some(agent_dir) = wardian_agent_dir {
        let projected_home = std::path::Path::new(agent_dir)
            .join("habitat")
            .join(".codex");
        if let Some(path) = codex_session_file_path_in(&projected_home, session_id) {
            return Some(path);
        }
    }

    let global_home = dirs::home_dir()?.join(".codex");
    codex_session_file_path_in(&global_home, session_id)
}

pub fn latest_codex_session_index_entry(
    wardian_session_id: &str,
) -> Result<Option<(String, String)>, String> {
    let wardian_home = get_wardian_home().ok_or("Could not resolve Wardian home")?;
    let index_path = wardian_home
        .join("agents")
        .join(wardian_session_id)
        .join("habitat")
        .join(".codex")
        .join("session_index.jsonl");

    if !index_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&index_path)
        .map_err(|e| format!("Failed to read Codex session index: {}", e))?;
    let latest = content
        .lines()
        .rev()
        .find_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }

            let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
            let session_id = parsed.get("id")?.as_str()?.trim();
            let updated_at = parsed.get("updated_at")?.as_str()?.trim();
            if session_id.is_empty() || updated_at.is_empty() {
                return None;
            }

            Some((session_id.to_string(), updated_at.to_string()))
        });

    Ok(latest)
}

fn codex_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    for line in lines.iter().rev() {
        let payload = line.get("payload")?;
        let payload_type = payload.get("type").and_then(|v| v.as_str())?;
        match payload_type {
            "exec_approval_request" => return Some("Action Needed".to_string()),
            "task_started" | "agent_message" | "exec_command_begin" | "exec_command_start" => {
                return Some("Processing...".to_string())
            }
            "task_complete" => return Some("Idle".to_string()),
            _ => {}
        }
    }
    None
}

fn claude_is_real_user_query(line: &serde_json::Value) -> bool {
    classify_claude_user_event(line) == ClaudeUserEventKind::RealQuery
}

fn claude_permission_hook_matches_session(event: &serde_json::Value, session_id: &str) -> bool {
    if session_id.trim().is_empty() {
        return false;
    }

    if event
        .get("session_id")
        .and_then(|v| v.as_str())
        .is_some_and(|sid| sid == session_id)
    {
        return true;
    }

    event
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .and_then(|path| std::path::Path::new(path).file_stem())
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem == session_id)
}

fn claude_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    for line in lines.iter().rev() {
        match line.get("type").and_then(|v| v.as_str()) {
            Some("system") => {
                if let Some("permission_request") = line.get("subtype").and_then(|v| v.as_str()) {
                    return Some("Action Needed".to_string());
                }
            }
            Some("assistant") => {
                let stop_reason = line
                    .get("message")
                    .and_then(|v| v.get("stop_reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if stop_reason == "end_turn" || stop_reason == "stop_sequence" {
                    return Some("Idle".to_string());
                }
                return Some("Processing...".to_string());
            }
            Some("progress") => return Some("Processing...".to_string()),
            Some("user") => {
                if claude_is_real_user_query(line) {
                    return Some("Processing...".to_string());
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
struct OpenCodeLogMetrics {
    query_count: usize,
    init_timestamp: Option<String>,
    status: Option<String>,
}


/// Find the newest OpenCode log file whose filesystem mtime is at or after
/// `spawn_time`.  OpenCode creates one log file per process invocation using
/// a timestamp-based name, so the file(s) created after the PTY was launched
/// belong to this session.
///
/// OpenCode sometimes spawns a background server subprocess that writes to a
/// SEPARATE log file roughly 1 s after the parent process starts.  The watcher
/// calls this function on every tick while `ses_id` is still unknown so it can
/// switch to that newer server log once it appears.
///
/// Returns the file with the highest mtime among all qualifying candidates so
/// that the server log (newer) wins over the parent log (older).
#[cfg(test)]
fn opencode_log_path_after(
    base: &std::path::Path,
    spawn_time: std::time::SystemTime,
) -> Option<std::path::PathBuf> {
    let mut candidates: Vec<_> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("log"))
        .filter_map(|p| {
            let mtime = p.metadata().ok()?.modified().ok()?;
            if mtime >= spawn_time {
                Some((p, mtime))
            } else {
                None
            }
        })
        .collect();

    // Newest mtime last → next_back() returns the most recently created file.
    candidates.sort_by_key(|(_, mtime)| *mtime);
    candidates.into_iter().next_back().map(|(p, _)| p)
}

/// Find the OpenCode log file for a session by content-searching for the
/// Wardian session UUID.  The UUID appears in log entries because
/// `OPENCODE_CONFIG` points to a config file whose path embeds the UUID.
///
/// Used for sessions recovered after an app restart (where no live watcher
/// is running).
fn opencode_log_path_in(base: &std::path::Path, session_id: &str) -> Option<std::path::PathBuf> {
    let mut candidates = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("log"))
        .collect::<Vec<_>>();

    candidates.sort();
    candidates.reverse();

    candidates.into_iter().find(|path| {
        std::fs::read_to_string(path)
            .map(|content| content.contains(session_id))
            .unwrap_or(false)
    })
}

/// Return the ordered list of directories where opencode writes its log files.
/// Tries platform-native data dirs first (Windows: %LOCALAPPDATA%, %APPDATA%),
/// then the XDG fallback (~/.local/share).
fn opencode_log_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        dirs.push(d.join("opencode").join("log"));
    }
    if let Some(d) = dirs::data_dir() {
        let p = d.join("opencode").join("log");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    if let Some(h) = dirs::home_dir() {
        let p = h.join(".local").join("share").join("opencode").join("log");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs
}

/// Extract the real opencode session ID (ses_xxx) from a log file.
/// Looks for `service=session id=ses_xxx ... created` lines and returns
/// the last one found (most recently created session in the log).
pub fn opencode_extract_created_session_id(log_path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(log_path).ok()?;
    content
        .lines()
        .filter(|line| line.contains("service=session") && line.contains("created"))
        .filter_map(|line| {
            line.split_whitespace()
                .find(|w| w.starts_with("id=ses_"))
                .and_then(|w| w.strip_prefix("id="))
                .map(|id| id.to_string())
        })
        .next_back()
}

/// Derive status and metrics from an opencode log.
///
/// Status is determined semantically from `service=session.prompt` markers:
/// - `exiting loop`  → the prompt loop finished → **Idle**
/// - `step=N loop`   → the prompt loop is active → **Processing…**
///
/// This avoids timestamp comparisons entirely, which would be unreliable
/// because opencode logs timestamps in local time while `now` is UTC.
#[cfg(test)]
fn opencode_metrics_from_log(content: &str, session_id: &str) -> OpenCodeLogMetrics {
    let mut metrics = OpenCodeLogMetrics::default();
    // true  = last session.prompt event was "exiting loop" (Idle)
    // false = last session.prompt event was "step=N loop"  (Processing)
    let mut last_prompt_exited = false;
    let mut saw_prompt = false;
    let mut saw_error = false;

    for line in content.lines() {
        if !line.contains(session_id) {
            continue;
        }

        if metrics.init_timestamp.is_none() {
            metrics.init_timestamp = line.split_whitespace().nth(1).map(|ts| ts.to_string());
        }

        if line.contains("service=session.prompt") {
            if line.contains("exiting loop") {
                last_prompt_exited = true;
                saw_prompt = true;
            } else if line.contains(" step=") {
                metrics.query_count += 1;
                last_prompt_exited = false;
                saw_prompt = true;
            }
            continue;
        }

        if line.starts_with("ERROR ") || line.contains(" ERROR ") {
            saw_error = true;
        }
    }

    metrics.status = if saw_error && !last_prompt_exited {
        Some("Error".to_string())
    } else if !saw_prompt {
        // No prompt activity yet — return None so we don't override a
        // status set by the PTY reader (e.g. "Pending…" or "Off").
        if metrics.init_timestamp.is_some() {
            Some("Idle".to_string())
        } else {
            None
        }
    } else if last_prompt_exited {
        Some("Idle".to_string())
    } else {
        Some("Processing...".to_string())
    };

    metrics
}

pub async fn get_all_metrics(state: &AppState) -> Vec<AgentTelemetry> {
    struct AgentSnapshot {
        session_id: String,
        provider: String,
        folder: String,
        resume_session: Option<String>,
        process_id: Option<u32>,
        query_count: std::sync::Arc<std::sync::Mutex<usize>>,
        init_timestamp: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        current_status: std::sync::Arc<std::sync::Mutex<String>>,
        last_output_at: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
        log_path: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    }

    let snapshots: Vec<AgentSnapshot> = {
        let agents = state.agents.lock().await;
        agents
            .iter()
            .map(|(sid, agent)| AgentSnapshot {
                session_id: sid.clone(),
                provider: agent.config.provider.clone(),
                folder: agent.config.folder.clone(),
                resume_session: agent.config.resume_session.clone(),
                process_id: agent.process_id,
                query_count: agent.query_count.clone(),
                init_timestamp: agent.init_timestamp.clone(),
                current_status: agent.current_status.clone(),
                last_output_at: agent.last_output_at.clone(),
                log_path: agent.log_path.clone(),
            })
            .collect()
    };

    let sys_metrics = state.system_metrics.clone();
    tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        let mut sys = sys_metrics.blocking_lock();
        sys.refresh_all();

        let mut children_map: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
        for (pid, process) in sys.processes() {
            if let Some(parent) = process.parent() {
                children_map.entry(parent).or_default().push(*pid);
            }
        }

        for snap in &snapshots {
            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut uptime = 0;

            if let Some(pid) = snap.process_id {
                let root_pid = sysinfo::Pid::from_u32(pid);
                fn sum_tree(
                    pid: sysinfo::Pid,
                    sys: &sysinfo::System,
                    cmap: &HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>,
                    cpu: &mut f32,
                    mem: &mut f64,
                    uptime: &mut u64,
                ) {
                    if let Some(p) = sys.process(pid) {
                        *cpu += p.cpu_usage();
                        *mem += p.memory() as f64 / 1_048_576.0;
                        *uptime = std::cmp::max(*uptime, p.run_time());
                    }
                    if let Some(children) = cmap.get(&pid) {
                        for &cpid in children {
                            sum_tree(cpid, sys, cmap, cpu, mem, uptime);
                        }
                    }
                }
                sum_tree(
                    root_pid,
                    &sys,
                    &children_map,
                    &mut cpu,
                    &mut mem,
                    &mut uptime,
                );
            }

            // Detect whether the agent process is still alive
            let process_alive = snap
                .process_id
                .map(|pid| sys.process(sysinfo::Pid::from_u32(pid)).is_some())
                .unwrap_or(false);

            let mut q_count = *snap.query_count.lock().unwrap();
            let mut i_ts = snap.init_timestamp.lock().unwrap().clone();
            let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());

            // Provider-aware log discovery
            if snap.provider == "opencode" {
                let opencode_session_id = snap
                    .resume_session
                    .as_deref()
                    .filter(|value| value.starts_with("ses_"))
                    .unwrap_or(&snap.session_id);
                *log_path_lock = Some(opencode_session_diff_path(opencode_session_id));
                if !log_path_lock.as_ref().is_some_and(|path| path.exists()) {
                    let log_dirs = opencode_log_dirs();
                    for dir in &log_dirs {
                        if let Some(path) = opencode_log_path_in(dir, opencode_session_id) {
                            *log_path_lock = Some(path);
                            break;
                        }
                    }
                }
            } else if log_path_lock.is_none() {
                match snap.provider.as_str() {
                    "codex" => {
                        let agent_home = get_wardian_home()
                            .map(|home| home.join("agents").join(&snap.session_id))
                            .filter(|path| path.exists())
                            .map(|path| path.to_string_lossy().to_string());
                        if let Some(path) =
                            codex_session_file_path(&snap.session_id, agent_home.as_deref())
                        {
                            *log_path_lock = Some(path);
                        }
                    }
                    "claude" => {
                        // Claude Code stores sessions at:
                        // ~/.claude/projects/<project_dir>/<session_id>.jsonl
                        // where <project_dir> is the workspace path with :\/. replaced by -
                        if let Some(home) = dirs::home_dir() {
                            let project_dir = claude_project_dir_name(&snap.folder);
                            let candidate = home
                                .join(".claude")
                                .join("projects")
                                .join(&project_dir)
                                .join(format!("{}.jsonl", snap.session_id));
                            if candidate.exists() {
                                *log_path_lock = Some(candidate);
                            }
                        }
                    }
                    _ => {
                        // Gemini: scan ~/.gemini/tmp for chat log files
                        if let Some(home) = dirs::home_dir() {
                            let tmp_dir = home.join(".gemini").join("tmp");
                            if let Ok(entries) = std::fs::read_dir(tmp_dir) {
                                for entry in entries.flatten() {
                                    let chat_dir = entry.path().join("chats");
                                    if let Ok(chat_files) = std::fs::read_dir(chat_dir) {
                                        for chat_file in chat_files.flatten() {
                                            if let Ok(content) =
                                                std::fs::read_to_string(chat_file.path())
                                            {
                                                if let Ok(p) =
                                                    serde_json::from_str::<serde_json::Value>(
                                                        &content,
                                                    )
                                                {
                                                    if p.get("sessionId").and_then(|v| v.as_str())
                                                        == Some(&snap.session_id)
                                                    {
                                                        *log_path_lock = Some(chat_file.path());
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if log_path_lock.is_some() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Provider-aware log parsing for status/query enrichment
            if let Some(ref path) = *log_path_lock {
                if let Ok(content) = std::fs::read_to_string(path) {
                    match snap.provider.as_str() {
                        "codex" => {
                            let lines: Vec<serde_json::Value> = content
                                .lines()
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();

                            q_count = lines
                                .iter()
                                .filter(|l| {
                                    l.get("type").and_then(|v| v.as_str()) == Some("event_msg")
                                        && l.get("payload")
                                            .and_then(|v| v.get("type"))
                                            .and_then(|v| v.as_str())
                                            == Some("user_message")
                                })
                                .count();

                            if let Some(meta) = lines.iter().find(|l| {
                                l.get("type").and_then(|v| v.as_str()) == Some("session_meta")
                            }) {
                                if let Some(ts) = meta
                                    .get("payload")
                                    .and_then(|v| v.get("timestamp"))
                                    .and_then(|v| v.as_str())
                                {
                                    i_ts = Some(ts.to_string());
                                }
                            }

                            if let Some(status) = codex_status_from_log(&lines) {
                                *snap.current_status.lock().unwrap() = status;
                            }
                        }
                        "claude" => {
                            // Claude logs are JSONL — one JSON object per line
                            let lines: Vec<serde_json::Value> = content
                                .lines()
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();

                            q_count = lines
                                .iter()
                                .filter(|l| {
                                    l.get("type").and_then(|v| v.as_str()) == Some("user")
                                        && claude_is_real_user_query(l)
                                })
                                .count();

                            if let Some(first) = lines.first() {
                                if let Some(ts) = first.get("timestamp").and_then(|v| v.as_str()) {
                                    i_ts = Some(ts.to_string());
                                } else if let Some(ts_num) =
                                    first.get("timestamp").and_then(|v| v.as_i64())
                                {
                                    // Fallback if timestamp is an epoch number
                                    if let Some(dt) =
                                        chrono::DateTime::from_timestamp_millis(ts_num)
                                    {
                                        i_ts = Some(
                                            dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                                        );
                                    }
                                }
                            }

                            let current_status = snap.current_status.lock().unwrap().clone();
                            if current_status != "Action Needed" {
                                if let Some(status) = claude_status_from_log(&lines) {
                                    *snap.current_status.lock().unwrap() = status;
                                }
                            }
                        }
                        "opencode" => {
                            // OpenCode status and query count come from PTY events now.
                            // Keep the discovered storage path for diagnostics only.
                        }
                        _ => {
                            // Gemini logs are a single JSON object with a messages array
                            if let Ok(p) = serde_json::from_str::<serde_json::Value>(&content) {
                                if let Some(msgs) = p.get("messages").and_then(|v| v.as_array()) {
                                    q_count = msgs
                                        .iter()
                                        .filter(|m| {
                                            m.get("type").and_then(|v| v.as_str()) == Some("user")
                                                || m.get("role").and_then(|v| v.as_str())
                                                    == Some("user")
                                        })
                                        .count();
                                }
                                if let Some(st) = p.get("startTime").and_then(|v| v.as_str()) {
                                    i_ts = Some(st.to_string());
                                }
                            }
                        }
                    }
                }
            }

            if q_count > 0 {
                *snap.query_count.lock().unwrap() = q_count;
            }
            if let Some(ts) = i_ts {
                *snap.init_timestamp.lock().unwrap() = Some(ts);
            }

            if snap.provider == "opencode" {
                let current_status = snap.current_status.lock().unwrap().clone();
                let last_output_at = *snap.last_output_at.lock().unwrap();
                if opencode_should_fallback_to_idle(
                    &current_status,
                    last_output_at,
                    std::time::SystemTime::now(),
                ) {
                    *snap.current_status.lock().unwrap() = "Idle".to_string();
                }
            }

            // If the process has terminated, force status to "Off" so the UI
            // doesn't stay stuck on "Processing..." or "Action Needed".
            if !process_alive && snap.process_id.is_some() {
                *snap.current_status.lock().unwrap() = "Off".to_string();
            }

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                current_status: snap.current_status.lock().unwrap().clone(),
                log_path: log_path_lock
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
            });
        }
        results
    })
    .await
    .unwrap_or_default()
}

pub fn get_all_agent_classes(_app: &AppHandle) -> Vec<AgentClassDefinition> {
    if let Some(app_dir) = get_wardian_home() {
        let classes_path = app_dir.join("classes.json");
        if let Ok(data) = std::fs::read_to_string(&classes_path) {
            return serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default();
        }
    }
    Vec::new()
}

pub fn save_classes(_app: &AppHandle, classes: &[AgentClassDefinition]) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or("No home dir")?;
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(app_dir.join("classes.json"), json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn init_agent_classes(app: &AppHandle) {
    if let Some(app_dir) = get_wardian_home() {
        let classes_dir = app_dir.join("classes");
        let _ = std::fs::create_dir_all(&classes_dir);
        let _ = std::fs::create_dir_all(app_dir.join("common/desk"));
        let _ = std::fs::create_dir_all(app_dir.join("common/lineages"));

        // Ensure Claude can discover skills from the canonical .agents/skills/ location
        ensure_claude_skills_link(&app_dir.join("common"));

        let classes_path = app_dir.join("classes.json");

        // Migration and Initialization
        if !classes_path.exists() {
            let mut defaults: Vec<AgentClassDefinition> =
                serde_json::from_str(include_str!("default_classes.json")).unwrap_or_default();
            for d in defaults.iter_mut() {
                d.is_default = true;
            }

            let custom_path = app_dir.join("custom_classes.json");
            if custom_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&custom_path) {
                    let mut custom = serde_json::from_str::<Vec<AgentClassDefinition>>(&data)
                        .unwrap_or_default();
                    for c in custom.iter_mut() {
                        c.is_default = false;
                    }
                    defaults.extend(custom);
                }
                // We've successfully merged. We could delete custom_classes.json here.
                let _ = std::fs::remove_file(&custom_path);
            }

            let _ = save_classes(app, &defaults);
        }

        let classes = get_all_agent_classes(app);
        for cls in &classes {
            let role_dir = classes_dir.join(&cls.name);
            let _ = std::fs::create_dir_all(&role_dir);

            // 1. Create AGENTS.md master file
            let agents_md_path = role_dir.join("AGENTS.md");
            if !agents_md_path.exists() {
                let content = if cls.is_default {
                    app.path()
                        .resolve(
                            format!("agent_prompts/{}.md", cls.name),
                            tauri::path::BaseDirectory::Resource,
                        )
                        .ok()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default()
                } else {
                    format!("# {} Agent\n\n{}\n", cls.name, cls.description)
                };
                let _ = std::fs::write(agents_md_path, content);
            }

            // 2. Symlink .claude/skills/ → .agents/skills/ for Claude discovery
            ensure_claude_skills_link(&role_dir);

            // 3. Create provider stub files for providers that do not read AGENTS.md directly
            for stub_name in &["GEMINI.md", "CLAUDE.md"] {
                let stub_path = role_dir.join(stub_name);
                if !stub_path.exists() {
                    let _ = std::fs::write(stub_path, "@AGENTS.md\n");
                }
            }
        }
    }
}

pub fn get_agent_class_default_instruction(app: &AppHandle, class_name: &str) -> Option<String> {
    app.path()
        .resolve(
            format!("agent_prompts/{}.md", class_name),
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
}

pub async fn kill_all_agents(state: &AppState) {
    let mut agents = state.agents.lock().await;
    #[allow(unused_mut)]
    for (sid, mut agent) in agents.drain() {
        log_debug(&format!("[Wardian] Killing session {}", sid));
        terminate_active_agent_process(&mut agent);
    }
    state.agent_order.lock().await.clear();
}

#[cfg(test)]
mod tests {
    use super::{
        claude_permission_hook_matches_session, claude_status_from_log,
        codex_bootstrap_launch_context, finalize_interactive_spawn_args, headless_provider_launch,
        extract_terminal_titles, opencode_should_fallback_to_idle, opencode_status_from_title,
        interactive_provider_args, interactive_provider_cwd, interactive_provider_launch,
        migrate_codex_bootstrap_home, opencode_interactive_env, opencode_log_path_after,
        opencode_log_path_in, opencode_metrics_from_log, opencode_runtime_config_content,
        strip_flag_value_pairs, strip_standalone_flag,
    };
    use crate::models::AgentConfig;
    use std::path::Path;

    #[test]
    fn extract_terminal_titles_reads_bel_and_st_sequences() {
        let chunk = "\u{1b}]0;OpenCode\u{7}x\u{1b}]2;OC | Working\u{1b}\\";

        assert_eq!(
            extract_terminal_titles(chunk),
            vec!["OpenCode".to_string(), "OC | Working".to_string()]
        );
    }

    #[test]
    fn opencode_title_maps_to_status() {
        assert_eq!(opencode_status_from_title("OpenCode"), Some("Idle"));
        assert_eq!(opencode_status_from_title("OC | Working"), Some("Processing..."));
        assert_eq!(
            opencode_status_from_title("OC | Action Required: approve tool"),
            Some("Action Needed")
        );
        assert_eq!(opencode_status_from_title(""), None);
    }

    #[test]
    fn opencode_idle_fallback_triggers_after_quiet_period() {
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        let last = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3);

        assert!(opencode_should_fallback_to_idle("Processing...", Some(last), now));
        assert!(!opencode_should_fallback_to_idle("Idle", Some(last), now));
    }

    #[test]
    fn codex_bootstrap_launch_context_is_stable_for_same_workspace() {
        let wardian_home = Path::new("C:/Users/test/.wardian");
        let workspace_cwd = Path::new("D:/Development/Wardian");

        let (_, first_bootstrap_home) = codex_bootstrap_launch_context(wardian_home, workspace_cwd);
        let (_, second_bootstrap_home) =
            codex_bootstrap_launch_context(wardian_home, workspace_cwd);

        assert_eq!(first_bootstrap_home, second_bootstrap_home);
    }

    #[test]
    fn migrate_codex_bootstrap_home_keeps_bootstrap_root_for_reuse() {
        let temp = tempfile::tempdir().expect("temp dir");
        let bootstrap_home = temp.path().join("bootstrap").join(".codex");
        let final_home = temp.path().join("final").join(".codex");

        std::fs::create_dir_all(&bootstrap_home).expect("create bootstrap home");
        std::fs::write(bootstrap_home.join("config.toml"), "config").expect("write config");
        std::fs::create_dir_all(bootstrap_home.join("sessions")).expect("create sessions dir");
        std::fs::write(
            bootstrap_home.join("sessions").join("session.jsonl"),
            "session",
        )
        .expect("write session file");

        migrate_codex_bootstrap_home(&bootstrap_home, &final_home).expect("migrate bootstrap home");

        assert!(bootstrap_home.exists());
        assert!(bootstrap_home.join("config.toml").exists());
        assert!(final_home.join("sessions").join("session.jsonl").exists());
    }

    #[test]
    fn codex_bootstrap_launch_context_uses_real_workspace_cwd() {
        let wardian_home = Path::new("C:/Users/test/.wardian");
        let workspace_cwd = Path::new("D:/Development/Wardian");
        let (provider_cwd, bootstrap_home) =
            codex_bootstrap_launch_context(wardian_home, workspace_cwd);

        assert_eq!(provider_cwd, workspace_cwd);
        assert!(bootstrap_home.starts_with(wardian_home.join("provider-bootstrap").join("codex")));
        assert_ne!(bootstrap_home, workspace_cwd);
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
    fn claude_status_from_log_ignores_local_commands_after_idle() {
        let lines = vec![
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "done" }],
                    "stop_reason": "end_turn"
                }
            }),
            serde_json::json!({ "type": "system", "subtype": "turn_duration" }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<local-command-caveat>Do not respond.</local-command-caveat>"
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<command-name>/model</command-name><command-message>model</command-message>"
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<local-command-stdout>Set model to Opus 4.6</local-command-stdout>"
                }
            }),
            serde_json::json!({ "type": "custom-title" }),
            serde_json::json!({ "type": "file-history-snapshot" }),
        ];

        assert_eq!(claude_status_from_log(&lines), Some("Idle".to_string()));
    }

    #[test]
    fn claude_status_from_log_treats_real_user_prompt_as_processing() {
        let lines = vec![
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "done" }],
                    "stop_reason": "end_turn"
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": "Please continue." }
            }),
        ];

        assert_eq!(
            claude_status_from_log(&lines),
            Some("Processing...".to_string())
        );
    }

    #[test]
    fn claude_permission_hook_ignores_other_transcript_sessions() {
        let event = serde_json::json!({
            "session_id": "other-session",
            "transcript_path": "C:\\Users\\tgemi\\.claude\\projects\\D--Development-Wardian\\other-session.jsonl",
            "tool_name": "Bash"
        });

        assert!(!claude_permission_hook_matches_session(
            &event,
            "expected-session"
        ));
    }

    #[test]
    fn claude_permission_hook_accepts_matching_transcript_session() {
        let event = serde_json::json!({
            "session_id": "expected-session",
            "transcript_path": "C:\\Users\\tgemi\\.claude\\projects\\D--Development-Wardian\\expected-session.jsonl",
            "tool_name": "Bash"
        });

        assert!(claude_permission_hook_matches_session(
            &event,
            "expected-session"
        ));
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
    fn opencode_log_path_finds_newest_matching_log() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let older = log_dir.join("2026-04-11T210615.log");
        let newer = log_dir.join("2026-04-11T210616.log");
        let unrelated = log_dir.join("2026-04-11T210617.log");

        std::fs::write(
            &older,
            r#"INFO  2026-04-11T21:06:15 +0ms service=default args=[\"attach\",\"http://127.0.0.1:57079\",\"--session\",\"ses_target\"] opencode"#,
        )
        .expect("write older log");
        std::fs::write(
            &newer,
            r#"INFO  2026-04-11T21:06:16 +0ms service=default args=[\"attach\",\"http://127.0.0.1:57079\",\"--session\",\"ses_target\"] opencode"#,
        )
        .expect("write newer log");
        std::fs::write(
            &unrelated,
            r#"INFO  2026-04-11T21:06:17 +0ms service=default args=[\"attach\",\"http://127.0.0.1:57079\",\"--session\",\"ses_other\"] opencode"#,
        )
        .expect("write unrelated log");

        let found = opencode_log_path_in(&log_dir, "ses_target").expect("matching log path");

        assert_eq!(found, newer);
    }

    #[test]
    fn opencode_log_path_after_returns_newest_file_created_after_spawn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        // Pre-existing log (before spawn — must be ignored).
        let old_file = log_dir.join("2026-04-12T100000.log");
        std::fs::write(&old_file, "old session log").expect("write old");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let spawn_time = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Parent log — created first after spawn.
        let parent_log = log_dir.join("2026-04-12T100001.log");
        std::fs::write(&parent_log, "parent process startup").expect("write parent");

        std::thread::sleep(std::time::Duration::from_millis(20));

        // Server log — created ~1 s later; should win because it has higher mtime.
        let server_log = log_dir.join("2026-04-12T100002.log");
        std::fs::write(&server_log, "server session events").expect("write server");

        let found = opencode_log_path_after(&log_dir, spawn_time)
            .expect("should find a log after spawn");

        assert_eq!(
            found, server_log,
            "should return the newest (server) log, not the parent log"
        );
    }

    #[test]
    fn opencode_log_path_after_returns_none_when_no_files_after_spawn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let file = log_dir.join("2026-04-12T100001.log");
        std::fs::write(&file, "session started").expect("write");

        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(opencode_log_path_after(&log_dir, future).is_none());
    }

    #[test]
    fn opencode_metrics_from_log_counts_session_prompt_steps() {
        let content = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:53 +1ms service=llm providerID=opencode sessionID=ses_target stream\n",
            "INFO  2026-03-30T07:36:02 +0ms service=session.prompt step=1 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:36:04 +0ms service=session.prompt step=0 sessionID=ses_other loop\n"
        );

        let metrics = opencode_metrics_from_log(content, "ses_target");

        assert_eq!(metrics.query_count, 2);
    }

    #[test]
    fn opencode_metrics_from_log_derives_processing_idle_and_error_status() {
        // Processing: last session.prompt event is "step=N loop" (no exiting yet)
        let processing = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:53 +1ms service=llm providerID=opencode sessionID=ses_target stream\n"
        );
        // Idle: last session.prompt event is "exiting loop"
        let idle = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:53 +1ms service=llm providerID=opencode sessionID=ses_target stream\n",
            "INFO  2026-03-30T07:35:56 +0ms service=session.prompt step=1 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:56 +0ms service=session.prompt sessionID=ses_target exiting loop\n"
        );
        // Error: ERROR line with no subsequent "exiting loop"
        let errored = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "ERROR 2026-03-30T07:35:54 +997ms service=llm providerID=opencode sessionID=ses_target error={\"error\":{}} stream error\n"
        );

        let processing_metrics = opencode_metrics_from_log(processing, "ses_target");
        let idle_metrics = opencode_metrics_from_log(idle, "ses_target");
        let errored_metrics = opencode_metrics_from_log(errored, "ses_target");

        assert_eq!(processing_metrics.status, Some("Processing...".to_string()));
        assert_eq!(idle_metrics.status, Some("Idle".to_string()));
        assert_eq!(errored_metrics.status, Some("Error".to_string()));
    }

    #[test]
    fn opencode_interactive_args_include_dir_for_real_workspace_anchor() {
        let workspace_cwd = Path::new("D:/Development/Wardian");

        let args = interactive_provider_args("opencode", workspace_cwd, workspace_cwd, Vec::new());

        assert_eq!(
            args,
            vec!["D:/Development/Wardian".to_string()]
        );
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
    fn opencode_runtime_config_content_uses_class_system_and_user_roots() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let wardian_home = temp.path().join(".wardian");
        let common = wardian_home.join("common");
        let class_dir = wardian_home.join("classes").join("Builder");
        let user_dir = temp.path().join("user-root");

        std::fs::create_dir_all(common.join(".agents").join("skills").join("common-skill"))
            .expect("common skill dir");
        std::fs::create_dir_all(class_dir.join(".agents").join("skills").join("class-skill"))
            .expect("class skill dir");
        std::fs::create_dir_all(user_dir.join(".agents").join("skills").join("user-skill"))
            .expect("user skill dir");
        std::fs::write(common.join("AGENTS.md"), "common").expect("common AGENTS");
        std::fs::write(class_dir.join("AGENTS.md"), "class").expect("class AGENTS");
        std::fs::write(user_dir.join("AGENTS.md"), "user").expect("user AGENTS");

        unsafe { std::env::set_var("WARDIAN_HOME", wardian_home.to_string_lossy().to_string()) };

        let config = AgentConfig {
            include_directories: Some(vec![user_dir.to_string_lossy().to_string()]),
            ..Default::default()
        };

        let content =
            opencode_runtime_config_content("Builder", None, Some(&config)).expect("config");

        unsafe { std::env::remove_var("WARDIAN_HOME") };

        let parsed: serde_json::Value = serde_json::from_str(&content).expect("json config");
        let instructions = parsed["instructions"]
            .as_array()
            .expect("instructions array");

        assert_eq!(instructions.len(), 3);
        assert!(parsed.get("skills").is_none());
    }

    #[test]
    fn opencode_interactive_env_includes_runtime_config_file_and_truecolor() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let wardian_home = temp.path().join(".wardian");
        let common = wardian_home.join("common");
        let class_dir = wardian_home.join("classes").join("Builder");
        let agent_dir = wardian_home.join("agents").join("ses_123");
        let user_dir = temp.path().join("user-root");

        std::fs::create_dir_all(common.join(".agents").join("skills").join("common-skill"))
            .expect("common skill dir");
        std::fs::create_dir_all(class_dir.join(".agents").join("skills").join("class-skill"))
            .expect("class skill dir");
        std::fs::create_dir_all(agent_dir.join(".agents").join("skills").join("agent-skill"))
            .expect("agent skill dir");
        std::fs::create_dir_all(user_dir.join(".agents").join("skills").join("user-skill"))
            .expect("user skill dir");
        std::fs::write(common.join("AGENTS.md"), "common").expect("common AGENTS");
        std::fs::write(class_dir.join("AGENTS.md"), "class").expect("class AGENTS");
        std::fs::write(agent_dir.join("AGENTS.md"), "agent").expect("agent AGENTS");
        std::fs::write(user_dir.join("AGENTS.md"), "user").expect("user AGENTS");

        unsafe { std::env::set_var("WARDIAN_HOME", wardian_home.to_string_lossy().to_string()) };

        let config = AgentConfig {
            session_id: "ses_123".into(),
            agent_class: "Builder".into(),
            include_directories: Some(vec![user_dir.to_string_lossy().to_string()]),
            ..Default::default()
        };

        let envs = opencode_interactive_env(Path::new("D:/Development/Wardian"), &config)
            .expect("interactive envs");

        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert!(envs.contains(&("COLORTERM".to_string(), "truecolor".to_string())));
        let config_path = envs
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG")
            .map(|(_, value)| value)
            .expect("interactive runtime config path");
        let parsed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(config_path).expect("read runtime config"),
        )
        .expect("json config");
        let instructions = parsed["instructions"]
            .as_array()
            .expect("instructions array");

        assert_eq!(instructions.len(), 4);

        // Config JSON must NOT contain skills.paths — OpenCode 1.4.3 does not
        // expose a skills.paths config key, so Wardian omits it entirely.
        assert!(
            parsed.get("skills").is_none(),
            "skills key must not be present in the config"
        );

        let config_dir = envs
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG_DIR")
            .map(|(_, value)| value)
            .expect("interactive runtime config dir");

        assert!(
            std::path::Path::new(config_dir)
                .join("skills")
                .join("common-skill")
                .exists(),
            "OPENCODE_CONFIG_DIR should expose projected skills"
        );
    }

    #[cfg(windows)]
    #[test]
    fn opencode_headless_launch_uses_cmd_host_on_windows() {
        let launch = headless_provider_launch(
            "opencode",
            "C:/nvm4w/nodejs/opencode",
            &[
                "run".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--dir".to_string(),
                "D:/Development/Wardian".to_string(),
                "Introduce yourself".to_string(),
            ],
        )
        .expect("headless launch spec");

        assert!(
            launch.executable.to_ascii_lowercase().ends_with("cmd.exe"),
            "expected cmd.exe host, got {}",
            launch.executable
        );
        assert_eq!(launch.args[0], "/d");
        assert_eq!(launch.args[1], "/c");
        assert!(launch.args[2].contains("opencode"));
        assert!(launch.args[2].contains("--format"));
        assert!(launch.args[2].contains("Introduce yourself"));
    }

    #[test]
    fn opencode_interactive_launch_bypasses_shell_wrapper() {
        let launch = interactive_provider_launch(
            "opencode",
            "C:/real/opencode.exe",
            &[
                "--session".to_string(),
                "ses_test".to_string(),
                "D:/Development/Wardian".to_string(),
            ],
        )
        .expect("interactive launch spec");

        assert_eq!(launch.executable, "C:/real/opencode.exe");
        assert_eq!(
            launch.args,
            vec![
                "--session".to_string(),
                "ses_test".to_string(),
                "D:/Development/Wardian".to_string(),
            ]
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
    fn codex_bootstrap_exec_mode_keeps_skip_git_repo_check() {
        let config = AgentConfig {
            provider: "codex".to_string(),
            codex_skip_git_repo_check: Some(true),
            ..Default::default()
        };

        let provider = crate::providers::ProviderFactory::resolve("codex").unwrap();
        let (_bin, mut provider_args) = provider.get_executable();
        provider_args.push("--cd".to_string());
        provider_args.push("D:/Development/Wardian".to_string());
        provider_args.push("exec".to_string());
        let spawn_args = strip_flag_value_pairs(provider.get_spawn_args(&config, false), "--add-dir");
        provider_args.extend(strip_standalone_flag(spawn_args, "--no-alt-screen"));
        if config.codex_skip_git_repo_check.unwrap_or(true) {
            provider_args.push("--skip-git-repo-check".to_string());
        }

        assert!(provider_args.contains(&"--skip-git-repo-check".to_string()));
        assert!(!provider_args.contains(&"--no-alt-screen".to_string()));
    }

    #[test]
    fn claude_interactive_spawn_drops_stream_json_flags() {
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
                "--resume".to_string(),
                "claude-session".to_string(),
            ],
        );

        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                "claude-session".to_string(),
            ]
        );
    }

    #[test]
    fn opencode_interactive_args_append_dir_after_flags() {
        let args = interactive_provider_args(
            "opencode",
            Path::new("C:/Users/test/.wardian/agents/ses_test/habitat"),
            Path::new("D:/Development/Wardian"),
            vec!["--session".to_string(), "ses_test".to_string()],
        );

        assert_eq!(
            args,
            vec![
                "--session".to_string(),
                "ses_test".to_string(),
                "C:/Users/test/.wardian/agents/ses_test/habitat/workspace".to_string(),
            ]
        );
    }

}
