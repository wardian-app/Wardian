use crate::{artifact_service::ArtifactPresentationAckV1, state::AppState};
use tauri::State;

/// Completes the exact pending presentation only after the Workbench routed it.
#[tauri::command]
pub async fn ack_artifact_presentation(
    ack: ArtifactPresentationAckV1,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.artifact_runtime.acknowledge(ack).await {
        Ok(())
    } else {
        Err("artifact presentation is no longer awaiting acknowledgement".to_string())
    }
}
