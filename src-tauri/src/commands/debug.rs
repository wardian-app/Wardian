use crate::state::AppState;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn debug_remove_agent_input_sender(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("debug commands are disabled in production builds".to_string());
    }

    let broker_state = state
        .terminal_sessions
        .broker_state(&session_id)
        .await
        .map_err(|error| error.to_string())?;
    state
        .terminal_sessions
        .pause_runtime(&session_id, broker_state.runtime_generation)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn debug_push_agent_watch_output(
    session_id: String,
    output: String,
    transcript_text: Option<String>,
    provider: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("debug commands are disabled in production builds".to_string());
    }

    let agents = state.agents.lock().await;
    let agent = agents
        .get(&session_id)
        .ok_or_else(|| format!("agent not found: {session_id}"))?;
    let mut watch_state = agent
        .watch_state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?;
    watch_state.push_output(output.as_bytes());
    if let Some(text) = transcript_text.filter(|text| !text.trim().is_empty()) {
        watch_state.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text,
            provider: provider.unwrap_or_else(|| "mock".to_string()),
            turn_id: Some("debug-seed".to_string()),
            source: Some("debug".to_string()),
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn debug_set_agent_status(
    session_id: String,
    status: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("debug commands are disabled in production builds".to_string());
    }

    let current_status = {
        let agents = state.agents.lock().await;
        agents
            .get(&session_id)
            .ok_or_else(|| format!("agent not found: {session_id}"))?
            .current_status
            .clone()
    };
    crate::manager::set_agent_status(&app, &session_id, &current_status, &status);
    Ok(())
}
