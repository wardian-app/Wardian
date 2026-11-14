//! Operator control wire for the chat domain's scoped archive adoption authority.
use super::{hold_release, ok_json, resolve_target_uuid, ControlError};
use crate::state::AppState;
use tauri::{AppHandle, Manager};
use wardian_core::control::ControlRequest;

pub(super) async fn dispatch(
    request: ControlRequest,
    app: &AppHandle,
) -> Result<String, ControlError> {
    let ControlRequest::AgentAdoptArchive {
        target,
        request,
        origin,
    } = request
    else {
        return Err(ControlError::bad_request("expected agent_adopt_archive"));
    };
    let agent_id = resolve_target_uuid(app, &target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    // Issuer attribution accompanies the recorded approval; it does not establish consent.
    let issuer = hold_release::operator_os_identity().map_err(ControlError::request_failed)?;
    let result = crate::commands::chat::authorize_archive_adoption(
        &app.state::<AppState>(),
        &agent_id,
        request,
        origin,
        issuer,
    )
    .await
    .map_err(ControlError::request_failed)?;
    let response = ok_json(&result)?;
    let app = app.clone();
    // Authorization is already durable. Capture runs independently of this IPC waiter.
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Err(error) = crate::commands::background_capture::capture_background_for_state(
            state.inner(),
            &agent_id,
        )
        .await
        {
            crate::manager::log_debug(&format!(
                "[WARDIAN] archive adoption capture failed for {agent_id}: {error}"
            ));
        }
    });
    Ok(response)
}
