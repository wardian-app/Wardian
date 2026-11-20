//! Route explicit agent controls to their existing lifecycle or grant owners.
use super::{agent_lifecycle, archive_adoption, hold_release, ControlError};
use tauri::AppHandle;
use wardian_core::control::ControlRequest;

pub(super) async fn dispatch(
    request: ControlRequest,
    app: &AppHandle,
) -> Result<String, ControlError> {
    match request {
        ControlRequest::AgentRename { target, name } => {
            agent_lifecycle::rename(app, &target, name).await
        }
        request @ ControlRequest::AgentReleaseHold { .. } => {
            hold_release::dispatch(request, app).await
        }
        request @ ControlRequest::AgentAdoptArchive { .. } => {
            archive_adoption::dispatch(request, app).await
        }
        ControlRequest::AgentRestart { target } | ControlRequest::AgentResume { target } => {
            agent_lifecycle::resume(app, &target).await
        }
        ControlRequest::AgentNewSession { target } => {
            agent_lifecycle::new_session(app, &target).await
        }
        ControlRequest::AgentPause { target } => agent_lifecycle::pause(app, &target).await,
        _ => Err(ControlError::bad_request("expected agent operator control")),
    }
}
