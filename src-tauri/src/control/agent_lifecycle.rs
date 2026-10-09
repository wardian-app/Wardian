//! Share the desktop lifecycle and its exclusion/error handling with the CLI.
use tauri::{AppHandle, Manager};
use wardian_core::control::OkResponse;

use super::{handle_agent_pause, ok_json, resolve_target_uuid, ControlError};
use crate::state::AppState;

async fn target_uuid(app: &AppHandle, target: &str) -> Result<String, ControlError> {
    resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))
}

pub(super) async fn resume(app: &AppHandle, target: &str) -> Result<String, ControlError> {
    let uuid = target_uuid(app, target).await?;
    crate::commands::agent::resume_agent(uuid, app.state::<AppState>(), app.clone())
        .await
        .map_err(ControlError::request_failed)?;
    ok_json(&OkResponse::new())
}

/// Use the UI clear operation verbatim, including starting previously Off agents.
pub(super) async fn new_session(app: &AppHandle, target: &str) -> Result<String, ControlError> {
    let uuid = target_uuid(app, target).await?;
    crate::commands::agent::clear_agent_session(uuid, None, app.state::<AppState>(), app.clone())
        .await
        .map_err(ControlError::request_failed)?;
    ok_json(&OkResponse::new())
}

pub(super) async fn pause(app: &AppHandle, target: &str) -> Result<String, ControlError> {
    let uuid = target_uuid(app, target).await?;
    handle_agent_pause(app, &uuid).await?;
    ok_json(&OkResponse::new())
}
