use crate::manager;
use crate::state::AppState;
use crate::utils::terminal_input::submit_prompt_via_sender;
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
                        if let Ok(mut status) = agent.current_status.lock() {
                            *status = "Idle".to_string();
                        }
                        let _ = app.emit(
                            "agent-status-updated",
                            serde_json::json!({
                                "session_id": session_id,
                                "current_status": "Idle",
                            }),
                        );
                    }
                } else if is_submit {
                    let agents = state.agents.lock().await;
                    if let Some(agent) = agents.get(&session_id) {
                        if agent.config.provider == "opencode" {
                            let current_status = agent
                                .current_status
                                .lock()
                                .map(|status| status.clone())
                                .unwrap_or_default();
                            if current_status != "Action Needed" && current_status != "Off" {
                                if let Ok(mut status) = agent.current_status.lock() {
                                    *status = "Processing...".to_string();
                                }
                                let _ = app.emit(
                                    "agent-status-updated",
                                    serde_json::json!({
                                        "session_id": session_id,
                                        "current_status": "Processing...",
                                    }),
                                );
                            }
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
        let tx = state
            .input_senders
            .try_read()
            .map_err(|_| "Input channel temporarily locked".to_string())?
            .get(&session_id)
            .cloned();
        (
            agent.config.provider.clone(),
            agent.config.clone(),
            tx,
        )
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
                if let Ok(mut count) = agent.query_count.lock() {
                    *count += 1;
                }
                if let Ok(mut status) = agent.current_status.lock() {
                    *status = "Processing...".to_string();
                }
            }
        }

        let _ = _app.emit(
            "agent-status-updated",
            serde_json::json!({
                "session_id": session_id,
                "current_status": "Processing...",
            }),
        );

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
                        if let Ok(mut status) = agent.current_status.lock() {
                            *status = "Error".to_string();
                        }
                    }
                }

                let _ = _app.emit(
                    "agent-status-updated",
                    serde_json::json!({
                        "session_id": session_id,
                        "current_status": "Error",
                    }),
                );

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
            if response_text.is_empty() { "<empty>" } else { response_text }
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
                if let Ok(mut status) = agent.current_status.lock() {
                    *status = "Idle".to_string();
                }
            }
        }

        let _ = _app.emit(
            "agent-status-updated",
            serde_json::json!({
                "session_id": session_id,
                "current_status": "Idle",
            }),
        );

        return Ok(());
    }

    submit_prompt_via_sender(&tx, &prompt).await
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

#[cfg(test)]
mod tests {
    #[test]
    fn terminal_prompt_normalization_flattens_newlines_for_submit() {
        let normalized =
            crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
                "Line one\nLine two\r\nLine three",
            );

        assert_eq!(normalized, "Line one Line two Line three");
    }

    #[test]
    fn terminal_prompt_normalization_trims_outer_whitespace() {
        let normalized =
            crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
                "   hello world  \r\n",
            );

        assert_eq!(normalized, "hello world");
    }
}
