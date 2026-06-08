use crate::manager;
use crate::state::{AppState, UserTerminalSession};
use crate::utils::append_bounded_pty_output;
use crate::utils::PtyUtf8Decoder;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;
use tauri::{AppHandle, Emitter};

#[cfg(test)]
pub(crate) async fn submit_prompt_to_agent_with_codex_echo_guard(
    state: &AppState,
    session_id: &str,
    provider_name: &str,
    tx: &tokio::sync::mpsc::Sender<Vec<u8>>,
    prompt: &str,
) -> Result<(), String> {
    let payload_cursor =
        crate::control::codex_payload_echo_cursor(state, provider_name, session_id).await;
    let wait_provider = provider_name.to_string();
    let wait_session_id = session_id.to_string();
    let wait_prompt = prompt.to_string();

    crate::utils::terminal_input::submit_prompt_with_outcome_via_sender_after_payload(
        tx,
        prompt,
        provider_name,
        || async move {
            crate::control::wait_for_codex_payload_echo_before_submit(
                state,
                &wait_provider,
                &wait_session_id,
                payload_cursor.as_deref(),
                &wait_prompt,
            )
            .await;
        },
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
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
                        if (provider == "opencode"
                            || provider == "gemini"
                            || provider == "antigravity")
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
    app: AppHandle,
) -> Result<wardian_core::control::DeliveryDetail, String> {
    crate::delivery::submit_live_surface_prompt(
        Some(&app),
        &state,
        crate::delivery::LiveSurfacePromptRequest::message(session_id, prompt),
    )
    .await
    .map_err(|error| error.to_string())
    .map(|result| result.detail)
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
    options: Option<ReadAgentPtyOptions>,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(&session_id) {
        if let Ok(mut buf) = agent.output_buffer.lock() {
            Ok(read_pty_buffer(&mut buf, options.as_ref()))
        } else {
            Ok(None)
        }
    } else {
        Err(format!("Agent {} not found", session_id))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReadAgentPtyOptions {
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub peek: bool,
}

fn read_pty_buffer(buffer: &mut String, options: Option<&ReadAgentPtyOptions>) -> Option<String> {
    if buffer.is_empty() {
        return None;
    }

    let output = match options
        .filter(|options| options.peek)
        .and_then(|options| options.max_bytes)
        .filter(|max_bytes| *max_bytes > 0)
    {
        Some(max_bytes) if buffer.len() > max_bytes => {
            pty_tail_from_line_boundary(buffer, max_bytes)
        }
        _ => buffer.clone(),
    };

    if !options.is_some_and(|options| options.peek) {
        buffer.clear();
    }

    Some(output)
}

fn pty_tail_from_line_boundary(buffer: &str, max_bytes: usize) -> String {
    let mut start = buffer.len().saturating_sub(max_bytes);
    while start < buffer.len() && !buffer.is_char_boundary(start) {
        start += 1;
    }

    if let Some(newline_offset) = buffer[start..].find('\n') {
        let line_start = start + newline_offset + 1;
        if line_start < buffer.len() {
            start = line_start;
        }
    }

    buffer[start..].to_string()
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
                        append_bounded_pty_output(&mut output, &text);
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
    use super::{
        read_pty_buffer, submit_prompt_to_agent_with_codex_echo_guard, validate_user_terminal_cwd,
        ReadAgentPtyOptions,
    };
    use crate::state::{ActiveAgent, AppState};
    use std::sync::{Arc, Mutex};
    use wardian_core::models::AgentConfig;

    #[test]
    fn user_terminal_cwd_validation_rejects_missing_directory() {
        let missing = std::env::temp_dir().join("wardian-missing-user-terminal-dir");
        let err = validate_user_terminal_cwd(&missing.to_string_lossy()).expect_err("missing dir");
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn terminal_prompt_normalization_preserves_newlines_for_submit() {
        let normalized = crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
            "Line one\nLine two\r\nLine three",
        );

        assert_eq!(normalized, "Line one\nLine two\nLine three");
    }

    #[test]
    fn terminal_prompt_normalization_trims_outer_whitespace() {
        let normalized = crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
            "   hello world  \r\n",
        );

        assert_eq!(normalized, "hello world");
    }

    #[tokio::test]
    async fn codex_submit_waits_for_prompt_echo_before_enter() {
        let state = AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            262_144,
        )));
        state.agents.lock().await.insert(
            "agent-1".to_string(),
            test_agent("agent-1", "codex", watch_state.clone()),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx.clone());

        let submit = tokio::spawn(async move {
            submit_prompt_to_agent_with_codex_echo_guard(
                &state,
                "agent-1",
                "codex",
                &tx,
                "Check composer injection",
            )
            .await
        });

        assert_eq!(
            rx.recv().await.expect("payload"),
            b"\x1b[200~Check composer injection\x1b[201~".to_vec()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
                .await
                .is_err(),
            "submit key should wait for the Codex prompt echo"
        );

        watch_state
            .lock()
            .unwrap()
            .push_output("› Check composer injection".as_bytes());

        assert_eq!(rx.recv().await.expect("submit key"), b"\r".to_vec());
        submit.await.expect("submit task").expect("submit succeeds");
    }

    #[test]
    fn read_pty_buffer_can_peek_a_bounded_recent_tail_without_draining() {
        let mut buffer = "older scrollback\nmiddle scrollback\nrecent frame\n".to_string();

        let peeked = read_pty_buffer(
            &mut buffer,
            Some(&ReadAgentPtyOptions {
                max_bytes: Some(24),
                peek: true,
            }),
        );

        assert_eq!(peeked.as_deref(), Some("recent frame\n"));
        assert_eq!(
            buffer,
            "older scrollback\nmiddle scrollback\nrecent frame\n"
        );

        let drained = read_pty_buffer(&mut buffer, None);

        assert_eq!(
            drained.as_deref(),
            Some("older scrollback\nmiddle scrollback\nrecent frame\n")
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn read_pty_buffer_ignores_bounds_for_destructive_reads() {
        let mut buffer = "older scrollback\nmiddle scrollback\nrecent frame\n".to_string();

        let drained = read_pty_buffer(
            &mut buffer,
            Some(&ReadAgentPtyOptions {
                max_bytes: Some(24),
                peek: false,
            }),
        );

        assert_eq!(
            drained.as_deref(),
            Some("older scrollback\nmiddle scrollback\nrecent frame\n")
        );
        assert!(buffer.is_empty());
    }

    fn test_agent(
        session_id: &str,
        provider: &str,
        watch_state: Arc<Mutex<crate::state::AgentWatchState>>,
    ) -> ActiveAgent {
        ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig {
                session_id: session_id.to_string(),
                session_name: session_id.to_string(),
                agent_class: "Coder".to_string(),
                provider: provider.to_string(),
                folder: "D:/work".to_string(),
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: Arc::new(Mutex::new(String::new())),
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Idle".to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state,
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }
}
