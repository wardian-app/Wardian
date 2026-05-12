use crate::manager;
use crate::state::{AppState, UserTerminalSession};
use crate::utils::terminal_input::submit_prompt_via_sender;
use crate::utils::PtyUtf8Decoder;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;
use tauri::{AppHandle, Emitter};

async fn wait_for_opencode_terminal_ready(
    session_id: &str,
    state: &AppState,
    timeout_ms: u64,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(timeout_ms) {
        let is_ready = {
            let agents = state.agents.lock().await;
            let agent = agents
                .get(session_id)
                .ok_or_else(|| format!("Agent {} not found or is off", session_id))?;
            let title = agent
                .terminal_title
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            title == "OpenCode" || title.starts_with("OC | ") || title.contains("Action Required")
        };

        if is_ready {
            manager::log_debug(&format!(
                "[Wardian] OpenCode terminal ready for session {}",
                session_id
            ));
            return Ok(());
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    manager::log_debug(&format!(
        "[Wardian] OpenCode terminal readiness timed out for session {}",
        session_id
    ));
    Err("Timed out waiting for OpenCode terminal to become ready".to_string())
}

#[tauri::command]
pub async fn send_input_to_agent(
    session_id: String,
    input: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let is_interrupt = input.contains('\u{3}');
    let is_submit = input.contains('\r') || input.contains('\n');
    let tx = match state.input_senders.try_read() {
        Ok(s) => s,
        Err(_) => {
            manager::log_debug(&format!(
                "[Wardian] [{}] send_input_to_agent: input_senders write-locked, dropping keystroke",
                session_id
            ));
            return Err("Input channel temporarily locked".to_string());
        }
    }
    .get(&session_id)
    .cloned();
    if let Some(tx) = tx {
        match tx.try_send(input.into_bytes()) {
            Ok(()) => {
                if is_interrupt {
                    let agents = state.agents.lock().await;
                    if let Some(agent) = agents.get(&session_id) {
                        manager::set_agent_status(&app, &session_id, &agent.current_status, "Idle");
                    }
                } else if is_submit {
                    let agents = state.agents.lock().await;
                    if let Some(agent) = agents.get(&session_id) {
                        let provider = agent.config.lock().unwrap().provider.clone();
                        if (provider == "opencode" || provider == "gemini")
                            && manager::mark_agent_prompt_started(agent)
                        {
                            manager::set_agent_status(
                                &app,
                                &session_id,
                                &agent.current_status,
                                "Processing...",
                            );
                        }
                    }
                }
                Ok(())
            }
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
pub async fn inject_session_input(
    session_id: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let senders = match state.input_senders.try_read() {
        Ok(s) => s,
        Err(_) => {
            manager::log_debug(&format!(
                "[Wardian] [{}] inject_session_input: input_senders write-locked",
                session_id
            ));
            return Err("Input channel temporarily locked".to_string());
        }
    };
    if let Some(tx) = senders.get(&session_id) {
        match tx.try_send(text.into_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                manager::log_debug(&format!(
                    "[Wardian] [{}] inject_session_input: channel error: {}",
                    session_id, e
                ));
                Err(format!("Failed to inject input: {}", e))
            }
        }
    } else {
        Err(format!("Agent {} not found or is off", session_id))
    }
}

#[tauri::command]
pub async fn submit_prompt_to_agent(
    session_id: String,
    prompt: String,
    state: State<'_, AppState>,
    _app: AppHandle,
) -> Result<(), String> {
    let (provider_name, config, tx) = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("Agent {} not found or is off", session_id))?;
        let config = agent.config.lock().unwrap().clone();
        let tx = state
            .input_senders
            .try_read()
            .map_err(|_| "Input channel temporarily locked".to_string())?
            .get(&session_id)
            .cloned();
        (config.provider.clone(), config, tx)
    };

    let tx = match tx {
        Some(tx) => tx,
        None => return Err(format!("Agent {} not found or is off", session_id)),
    };

    if provider_name == "opencode" {
        wait_for_opencode_terminal_ready(&session_id, &state, 15000).await?;

        {
            let agents = state.agents.lock().await;
            if let Some(agent) = agents.get(&session_id) {
                if manager::mark_agent_prompt_started(agent) {
                    manager::set_agent_status(
                        &_app,
                        &session_id,
                        &agent.current_status,
                        "Processing...",
                    );
                }
            }
        }

        let habitat_cwd = crate::utils::fs::get_wardian_home()
            .map(|home| home.join("agents").join(&session_id).join("habitat"))
            .filter(|path| path.exists())
            .unwrap_or_else(|| crate::utils::fs::resolve_cwd(&config.folder, &session_id));

        let result = match manager::run_headless_with_config(
            &habitat_cwd,
            &prompt,
            &session_id,
            "text",
            &provider_name,
            Some(&config),
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                {
                    let agents = state.agents.lock().await;
                    if let Some(agent) = agents.get(&session_id) {
                        manager::set_agent_status(
                            &_app,
                            &session_id,
                            &agent.current_status,
                            "Error",
                        );
                    }
                }

                return Err(error);
            }
        };

        let response_text = result
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("");

        manager::log_debug(&format!(
            "[Wardian] OpenCode headless submit response for session {}: {}",
            session_id,
            if response_text.is_empty() {
                "<empty>"
            } else {
                response_text
            }
        ));

        if !response_text.is_empty() {
            let agents = state.agents.lock().await;
            if let Some(agent) = agents.get(&session_id) {
                if let Ok(mut buf) = agent.output_buffer.lock() {
                    if !buf.is_empty() && !buf.ends_with('\n') {
                        buf.push_str("\r\n");
                    }
                    buf.push_str(response_text);
                    buf.push_str("\r\n");
                }
            }
            let _ = _app.emit(
                "agent-pty-output-ready",
                serde_json::json!({ "session_id": session_id }),
            );
        }

        {
            let agents = state.agents.lock().await;
            if let Some(agent) = agents.get(&session_id) {
                manager::set_agent_status(&_app, &session_id, &agent.current_status, "Idle");
            }
        }

        return Ok(());
    }

    {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&session_id) {
            if manager::mark_agent_prompt_started(agent) {
                manager::set_agent_status(
                    &_app,
                    &session_id,
                    &agent.current_status,
                    "Processing...",
                );
            }
        }
    }

    submit_prompt_via_sender(&tx, &prompt, &provider_name).await
}

#[tauri::command]
pub async fn broadcast_input(input: String, state: State<'_, AppState>) -> Result<(), String> {
    let senders = state
        .input_senders
        .read()
        .map_err(|e| format!("Lock poisoned: {}", e))?;
    for tx in senders.values() {
        let _ = tx.try_send(input.clone().into_bytes());
    }
    Ok(())
}

#[tauri::command]
pub async fn send_binary_input_to_agent(
    session_id: String,
    input: Vec<u8>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let tx = match state.input_senders.try_read() {
        Ok(s) => s,
        Err(_) => {
            manager::log_debug(&format!(
                "[Wardian] [{}] send_binary_input_to_agent: input_senders write-locked, dropping binary input",
                session_id
            ));
            return Err("Input channel temporarily locked".to_string());
        }
    }
    .get(&session_id)
    .cloned();

    if let Some(tx) = tx {
        match tx.try_send(input) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                manager::log_debug(&format!(
                    "[Wardian] [{}] send_binary_input_to_agent: channel FULL (writer thread likely blocked on ConPTY write_all)",
                    session_id
                ));
                Err("Terminal input buffer full - PTY may be stalled".to_string())
            }
            Err(e) => {
                manager::log_debug(&format!(
                    "[Wardian] [{}] send_binary_input_to_agent: channel error: {}",
                    session_id, e
                ));
                Err(format!("Failed to send binary input: {}", e))
            }
        }
    } else {
        Err(format!("Agent {} not found or is off", session_id))
    }
}

#[tauri::command]
pub async fn resize_agent_terminal(
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
pub async fn read_agent_pty(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(&session_id) {
        if let Ok(mut buf) = agent.output_buffer.lock() {
            if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(std::mem::take(&mut *buf)))
            }
        } else {
            Ok(None)
        }
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

fn normalized_user_terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(4),
        cols: cols.max(10),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn validate_user_terminal_cwd(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Workspace path is required".to_string());
    }

    let path = PathBuf::from(trimmed);
    if !path.exists() || !path.is_dir() {
        return Err(format!("Workspace path does not exist: {}", path.display()));
    }

    Ok(path)
}

fn spawn_user_terminal_session(
    app: AppHandle,
    cols: u16,
    rows: u16,
) -> Result<UserTerminalSession, String> {
    let launch = crate::utils::build_interactive_shell_launch()?;
    let cwd = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    let session_id = uuid::Uuid::new_v4().to_string();

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(normalized_user_terminal_size(cols, rows))
        .map_err(|e| format!("Failed to open user terminal PTY: {}", e))?;

    let mut cmd = CommandBuilder::new(&launch.executable);
    for arg in &launch.args {
        cmd.arg(arg);
    }
    cmd.cwd(&cwd);
    manager::apply_terminal_identity_env(&mut cmd);

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn user terminal: {}", e))?;
    let process_id = child.process_id();

    #[cfg(windows)]
    let job_object = {
        if manager::app_process_supervisor_active() {
            None
        } else if let Ok(job) = manager::create_kill_on_close_job("user terminal fallback") {
            if let Some(pid) = process_id {
                if let Err(err) = manager::assign_pid_to_job(&job, pid, "user terminal fallback") {
                    manager::log_debug(&format!(
                        "[Wardian] Failed to assign user terminal PID {} to fallback job: {}",
                        pid, err
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
        .map_err(|e| format!("Failed to get user terminal PTY reader: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get user terminal PTY writer: {}", e))?;
    let pty_master = Arc::new(Mutex::new(pair.master));
    drop(pair.slave);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let output_buffer = Arc::new(Mutex::new(String::new()));
    let output_buffer_for_reader = output_buffer.clone();
    let exited = Arc::new(Mutex::new(false));
    let exited_for_reader = exited.clone();
    let app_for_reader = app.clone();
    let session_id_for_reader = session_id.clone();

    std::thread::spawn(move || {
        while let Some(input) = rx.blocking_recv() {
            if writer.write_all(&input).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut pty_decoder = PtyUtf8Decoder::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    if let Ok(mut is_exited) = exited_for_reader.lock() {
                        *is_exited = true;
                    }
                    let _ = app_for_reader.emit(
                        "user-terminal-exited",
                        serde_json::json!({ "session_id": session_id_for_reader }),
                    );
                    break;
                }
                Ok(n) => {
                    let text = pty_decoder.decode_chunk(&buf[..n]);
                    let should_emit = if let Ok(mut output) = output_buffer_for_reader.lock() {
                        let was_empty = output.is_empty();
                        output.push_str(&text);
                        was_empty
                    } else {
                        false
                    };
                    if should_emit {
                        let _ = app_for_reader.emit("user-terminal-output-ready", ());
                    }
                }
                Err(error) => {
                    manager::log_debug(&format!(
                        "[Wardian] User terminal PTY read error: {}",
                        error
                    ));
                    if let Ok(mut is_exited) = exited_for_reader.lock() {
                        *is_exited = true;
                    }
                    let _ = app_for_reader.emit(
                        "user-terminal-exited",
                        serde_json::json!({ "session_id": session_id_for_reader }),
                    );
                    break;
                }
            }
        }
    });

    Ok(UserTerminalSession {
        session_id,
        shell_id: launch.shell_id,
        child_process: Some(child),
        pty_master,
        stdin_tx: tx,
        output_buffer,
        process_id,
        exited,
        #[cfg(windows)]
        job_object,
    })
}

async fn resize_user_terminal_master(
    master_arc: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let size = normalized_user_terminal_size(cols, rows);
    tokio::task::spawn_blocking(move || {
        let master = match master_arc.lock() {
            Ok(master) => master,
            Err(poisoned) => poisoned.into_inner(),
        };
        master.resize(size)
    })
    .await
    .map_err(|e| format!("Failed to join user terminal resize task: {}", e))?
    .map_err(|e| format!("Failed to resize user terminal: {}", e))
}

fn user_terminal_exited(session: &UserTerminalSession) -> bool {
    session.exited.lock().map(|value| *value).unwrap_or(true)
}

#[tauri::command]
pub async fn ensure_user_terminal(
    cols: u16,
    rows: u16,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let (existing_master, current_session_id) = {
        let mut session = state.user_terminal.lock().await;
        if session.as_ref().is_some_and(user_terminal_exited) {
            session.take();
        }

        if let Some(terminal) = session.as_ref() {
            (
                Some(terminal.pty_master.clone()),
                Some(terminal.session_id.clone()),
            )
        } else {
            let terminal = spawn_user_terminal_session(app, cols, rows)?;
            let session_id = terminal.session_id.clone();
            *session = Some(terminal);
            (None, Some(session_id))
        }
    };

    if let Some(master) = existing_master {
        resize_user_terminal_master(master, cols, rows).await?;
    }

    current_session_id.ok_or("User terminal is not running".to_string())
}

#[tauri::command]
pub async fn send_input_to_user_terminal(
    input: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    send_binary_input_to_user_terminal(input.into_bytes(), state).await
}

#[tauri::command]
pub async fn send_binary_input_to_user_terminal(
    input: Vec<u8>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let tx = {
        let session = state.user_terminal.lock().await;
        let terminal = session
            .as_ref()
            .ok_or("User terminal is not running".to_string())?;
        if user_terminal_exited(terminal) {
            return Err("User terminal has exited".to_string());
        }
        terminal.stdin_tx.clone()
    };

    match tx.try_send(input) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            Err("User terminal input buffer full".to_string())
        }
        Err(error) => Err(format!("Failed to send user terminal input: {}", error)),
    }
}

#[tauri::command]
pub async fn resize_user_terminal(
    cols: u16,
    rows: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let master = {
        let session = state.user_terminal.lock().await;
        let terminal = session
            .as_ref()
            .ok_or("User terminal is not running".to_string())?;
        terminal.pty_master.clone()
    };
    resize_user_terminal_master(master, cols, rows).await
}

#[tauri::command]
pub async fn read_user_terminal_pty(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let buffer = {
        let session = state.user_terminal.lock().await;
        let terminal = session
            .as_ref()
            .ok_or("User terminal is not running".to_string())?;
        terminal.output_buffer.clone()
    };

    let result = if let Ok(mut output) = buffer.lock() {
        if output.is_empty() {
            Ok(None)
        } else {
            Ok(Some(std::mem::take(&mut *output)))
        }
    } else {
        Ok(None)
    };
    result
}

#[tauri::command]
pub async fn restart_user_terminal(
    cols: u16,
    rows: u16,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    {
        let mut session = state.user_terminal.lock().await;
        session.take();
    }
    ensure_user_terminal(cols, rows, state, app).await
}

#[tauri::command]
pub async fn set_user_terminal_cwd(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let path = validate_user_terminal_cwd(&path)?;
    let (tx, command) = {
        let session = state.user_terminal.lock().await;
        let terminal = session
            .as_ref()
            .ok_or("User terminal is not running".to_string())?;
        if user_terminal_exited(terminal) {
            return Err("User terminal has exited".to_string());
        }
        (
            terminal.stdin_tx.clone(),
            crate::utils::build_shell_cd_command(&terminal.shell_id, &path),
        )
    };

    match tx.try_send(command.into_bytes()) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            Err("User terminal input buffer full".to_string())
        }
        Err(error) => Err(format!(
            "Failed to change user terminal directory: {}",
            error
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_user_terminal_cwd;

    #[test]
    fn user_terminal_cwd_validation_rejects_missing_directory() {
        let missing = std::env::temp_dir().join("wardian-missing-user-terminal-dir");
        let err = validate_user_terminal_cwd(&missing.to_string_lossy()).expect_err("missing dir");
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn terminal_prompt_normalization_flattens_newlines_for_submit() {
        let normalized = crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
            "Line one\nLine two\r\nLine three",
        );

        assert_eq!(normalized, "Line one Line two Line three");
    }

    #[test]
    fn terminal_prompt_normalization_trims_outer_whitespace() {
        let normalized = crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
            "   hello world  \r\n",
        );

        assert_eq!(normalized, "hello world");
    }
}
