use crate::state::AppState;
use tauri::{AppHandle, State};

#[cfg(debug_assertions)]
use crate::state::UserFileGrantV1;
#[cfg(debug_assertions)]
use serde::Serialize;
#[cfg(debug_assertions)]
use wardian_core::files::FileResourceErrorV1;

#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DebugFileResourceStatsV1 {
    pub schema: u8,
    pub watcher_count: usize,
    pub subscriber_count: usize,
    pub ticket_count: usize,
    pub renderer_lease_count: usize,
    pub user_grant_count: usize,
}

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
        .terminate_and_remove_runtime(&session_id, broker_state.runtime_generation)
        .await
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

/// Creates the same exact-file grant as the native picker without opening an
/// operating-system dialog. This command is absent from release builds and is
/// reserved for the native E2E harness.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn debug_grant_file_resource_for_e2e(
    path: String,
    state: State<'_, AppState>,
) -> Result<UserFileGrantV1, FileResourceErrorV1> {
    crate::commands::files::record_picked_file(&state.file_resources, std::path::Path::new(&path))
        .await
}

/// Reports only aggregate file-runtime ownership counts needed to prove native
/// E2E cleanup. It exposes no paths, bytes, capabilities, or ticket secrets and
/// is absent from release builds.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn debug_file_resource_stats(
    resource_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<DebugFileResourceStatsV1, String> {
    Ok(DebugFileResourceStatsV1 {
        schema: 1,
        watcher_count: state.file_resources.watcher_count().await,
        subscriber_count: match resource_id {
            Some(resource_id) => state.file_resources.subscriber_count(&resource_id).await,
            None => 0,
        },
        ticket_count: state.file_resources.ticket_count().await,
        renderer_lease_count: state.file_resources.renderer_lease_count().await,
        user_grant_count: state.file_resources.user_grant_count().await,
    })
}
