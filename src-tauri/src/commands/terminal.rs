use crate::manager;
use crate::state::AppState;
use tauri::State;
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub async fn send_input_to_agent(
    session_id: String,
    input: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let is_interrupt = input.contains('\u{3}');
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
