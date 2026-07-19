use crate::{artifact_service::ArtifactPresentationAckV1, state::AppState};
use serde::{Deserialize, Serialize};
use tauri::State;
use wardian_core::{
    artifacts::{ArtifactManifestV1, ArtifactStore, ArtifactVersionV1},
    files::{AuthorizedRootService, FileResourceLimits, VerifiedFileSnapshot},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GetArtifactResourceRequestV1 {
    pub artifact_id: String,
    pub selected_version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactWorkingResourceV1 {
    pub canonical_path: String,
    pub agent_id: String,
    pub content_hash: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactResourceV1 {
    pub schema: u8,
    pub manifest: ArtifactManifestV1,
    pub selected_version: ArtifactVersionV1,
    pub selected_text: Option<String>,
    pub working: ArtifactWorkingResourceV1,
    pub attention: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct MarkArtifactAttentionReadRequestV1 {
    pub artifact_id: String,
}

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

fn artifact_store() -> Result<ArtifactStore, String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not locate Wardian home".to_string())?;
    ArtifactStore::open(home.join("artifacts")).map_err(|error| error.to_string())
}

/// Resolves one immutable presentation plus the currently authorized working file.
#[tauri::command]
pub async fn get_artifact_resource(
    request: GetArtifactResourceRequestV1,
    state: State<'_, AppState>,
) -> Result<ArtifactResourceV1, String> {
    let store = artifact_store()?;
    let stored = store
        .load_version(&request.artifact_id, request.selected_version_id.as_deref())
        .map_err(|error| error.to_string())?;
    let selected_bytes = store
        .read_version_bytes(&request.artifact_id, Some(&stored.version.version_id))
        .map_err(|error| error.to_string())?;
    let selected_text = String::from_utf8(selected_bytes).ok();
    let agent_id = stored.manifest.origin.session_id.clone();
    let config = {
        let agents = state.agents.lock().await;
        agents
            .get(&agent_id)
            .and_then(|agent| agent.config.lock().ok().map(|value| value.clone()))
    };
    let (content_hash, unavailable_reason) = match config {
        Some(config) => AuthorizedRootService::from_agent_config(&config)
            .and_then(|roots| {
                roots.authorize_existing_file(std::path::Path::new(&stored.manifest.canonical_path))
            })
            .and_then(|authorized| {
                VerifiedFileSnapshot::from_authorized_path(
                    &authorized,
                    &FileResourceLimits::default(),
                )
            })
            .map(|snapshot| (Some(snapshot.descriptor().content_hash.clone()), None))
            .unwrap_or_else(|error| (None, Some(error.message))),
        None => (
            None,
            Some("origin agent authorization is not currently available".to_string()),
        ),
    };
    let attention = store
        .list_recent(usize::MAX)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| entry.artifact_id == request.artifact_id)
        .is_some_and(|entry| entry.attention);
    Ok(ArtifactResourceV1 {
        schema: 1,
        working: ArtifactWorkingResourceV1 {
            canonical_path: stored.manifest.canonical_path.clone(),
            agent_id,
            content_hash,
            unavailable_reason,
        },
        manifest: stored.manifest,
        selected_version: stored.version,
        selected_text,
        attention,
    })
}

/// Clears only the actionable badge; the artifact thread and history remain durable.
#[tauri::command]
pub async fn mark_artifact_attention_read(
    request: MarkArtifactAttentionReadRequestV1,
) -> Result<(), String> {
    artifact_store()?
        .set_attention(&request.artifact_id, false)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
