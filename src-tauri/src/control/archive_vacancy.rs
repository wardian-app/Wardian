//! Operator wire to the archive state's authoritative vacancy repair method.
use super::{ok_json, resolve_target_uuid, ControlError};
use crate::state::AppState;
use tauri::{AppHandle, Manager};
use wardian_core::control::ControlRequest;

pub(super) async fn dispatch(
    request: ControlRequest,
    app: &AppHandle,
) -> Result<String, ControlError> {
    let ControlRequest::AgentArchiveVacancy { target, request } = request else {
        return Err(ControlError::bad_request("expected agent_archive_vacancy"));
    };
    request.validate().map_err(ControlError::bad_request)?;
    let agent_id = resolve_target_uuid(app, &target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let state = app.state::<AppState>();
    let result = state
        .conversation_archive
        .authorize_vacancy(&agent_id, request)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::InvalidInput => ControlError::bad_request(error.to_string()),
            _ => ControlError::request_failed(error),
        })?;
    ok_json(&result)
}
