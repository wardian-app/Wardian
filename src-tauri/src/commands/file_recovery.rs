//! Narrow Tauri commands for durable editor recovery and authorized merging.

use crate::state::{
    AppState, FileRecoveryCheckpointV1, FileRecoveryMergeResultV1, FileRecoverySummaryV1,
    FileRecoveryV1,
};
use serde::{Deserialize, Serialize};
use wardian_core::files::FileResourceErrorV1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CheckpointFileRecoveryRequestV1 {
    pub recovery_id: Option<String>,
    pub expected_recovery_revision: Option<u64>,
    pub resource_id: String,
    pub subscription_id: String,
    pub base_content_hash: String,
    /// Exact retained editor base whose digest is `base_content_hash`.
    pub base: String,
    pub resource_key: String,
    pub buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GetFileRecoveryRequestV1 {
    pub recovery_id: String,
    pub resource_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ListFileRecoveriesRequestV1 {
    pub resource_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DiscardFileRecoveryRequestV1 {
    pub recovery_id: String,
    pub expected_recovery_revision: u64,
    pub resource_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct MergeFileRecoveryRequestV1 {
    pub recovery_id: String,
    pub expected_recovery_revision: u64,
    pub resource_key: String,
    pub resource_id: String,
    pub subscription_id: String,
}

/// Checkpoints one dirty buffer and its exact hash-verified editor base using
/// the calling WebView label as recovery scope. Scope is never accepted from
/// serialized frontend input.
#[tauri::command]
pub async fn checkpoint_file_recovery(
    request: CheckpointFileRecoveryRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<FileRecoveryCheckpointV1, FileResourceErrorV1> {
    state
        .file_resources
        .checkpoint_recovery(
            request.recovery_id.as_deref(),
            request.expected_recovery_revision,
            &request.resource_id,
            &request.subscription_id,
            &request.base_content_hash,
            &request.base,
            &request.resource_key,
            webview.label(),
            &request.buffer,
        )
        .await
}

/// Reads only stored recovery base/buffer bytes under the calling WebView.
#[tauri::command]
pub async fn get_file_recovery(
    request: GetFileRecoveryRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<FileRecoveryV1, FileResourceErrorV1> {
    state
        .file_resources
        .get_recovery(&request.recovery_id, &request.resource_key, webview.label())
        .await
}

/// Discovers body-free recovery metadata under the calling WebView. Scope is
/// injected by Tauri and cannot be forged in request JSON.
#[tauri::command]
pub async fn list_file_recoveries(
    request: ListFileRecoveriesRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<Vec<FileRecoverySummaryV1>, FileResourceErrorV1> {
    state
        .file_resources
        .list_recoveries(&request.resource_key, webview.label())
        .await
}

/// Discards one exact recovery generation under the calling WebView.
#[tauri::command]
pub async fn discard_file_recovery(
    request: DiscardFileRecoveryRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<(), FileResourceErrorV1> {
    state
        .file_resources
        .discard_recovery(
            &request.recovery_id,
            request.expected_recovery_revision,
            &request.resource_key,
            webview.label(),
        )
        .await
}

/// Merges recovery bytes with a current disk head read through a newly
/// verified live subscription. The calling WebView supplies recovery scope.
#[tauri::command]
pub async fn merge_file_recovery(
    request: MergeFileRecoveryRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<FileRecoveryMergeResultV1, FileResourceErrorV1> {
    state
        .file_resources
        .merge_recovery(
            &request.recovery_id,
            request.expected_recovery_revision,
            &request.resource_key,
            webview.label(),
            &request.resource_id,
            &request.subscription_id,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_recovery_requests_are_strict_snake_case_without_scope_input() {
        let checkpoint: CheckpointFileRecoveryRequestV1 =
            serde_json::from_value(serde_json::json!({
                "recovery_id": null,
                "expected_recovery_revision": null,
                "resource_id": "file:/workspace/readme.md",
                "subscription_id": "subscription-a",
                "base_content_hash": "sha256:base",
                "base": "saved base",
                "resource_key": "file:/workspace/readme.md",
                "buffer": "edited"
            }))
            .expect("checkpoint request");
        let serialized = serde_json::to_value(&checkpoint).expect("serialize checkpoint");
        assert_eq!(serialized["base"], "saved base");
        assert!(serialized.get("webview_scope").is_none());
        assert!(serialized.get("expectedRecoveryRevision").is_none());

        let missing_base =
            serde_json::from_value::<CheckpointFileRecoveryRequestV1>(serde_json::json!({
                "recovery_id": null,
                "expected_recovery_revision": null,
                "resource_id": "file:/workspace/readme.md",
                "subscription_id": "subscription-a",
                "base_content_hash": "sha256:base",
                "resource_key": "file:/workspace/readme.md",
                "buffer": "edited"
            }));
        assert!(missing_base.is_err());
        let camel_case_base =
            serde_json::from_value::<CheckpointFileRecoveryRequestV1>(serde_json::json!({
                "recovery_id": null,
                "expected_recovery_revision": null,
                "resource_id": "file:/workspace/readme.md",
                "subscription_id": "subscription-a",
                "base_content_hash": "sha256:base",
                "baseText": "saved base",
                "resource_key": "file:/workspace/readme.md",
                "buffer": "edited"
            }));
        assert!(camel_case_base.is_err());
        let legacy_base_revision =
            serde_json::from_value::<CheckpointFileRecoveryRequestV1>(serde_json::json!({
                "recovery_id": null,
                "expected_recovery_revision": null,
                "resource_id": "file:/workspace/readme.md",
                "subscription_id": "subscription-a",
                "base_revision": 3,
                "base_content_hash": "sha256:base",
                "base": "saved base",
                "resource_key": "file:/workspace/readme.md",
                "buffer": "edited"
            }));
        assert!(legacy_base_revision.is_err());

        let scope_injection =
            serde_json::from_value::<GetFileRecoveryRequestV1>(serde_json::json!({
                "recovery_id": "recovery-a",
                "resource_key": "file:/workspace/readme.md",
                "webview_scope": "forged"
            }));
        assert!(scope_injection.is_err());

        let list: ListFileRecoveriesRequestV1 = serde_json::from_value(serde_json::json!({
            "resource_key": "file:/workspace/readme.md"
        }))
        .expect("list request");
        assert_eq!(list.resource_key, "file:/workspace/readme.md");
        let forged_list_scope =
            serde_json::from_value::<ListFileRecoveriesRequestV1>(serde_json::json!({
                "resource_key": "file:/workspace/readme.md",
                "webview_scope": "forged"
            }));
        assert!(forged_list_scope.is_err());
    }

    #[test]
    fn file_recovery_merge_outcomes_are_explicitly_tagged() {
        let clean = FileRecoveryMergeResultV1::Clean {
            recovery_revision: 2,
            current_revision: 4,
            current_content_hash: "sha256:current".to_string(),
            disk_changed: true,
            merged_text: "merged".to_string(),
        };
        let conflicted = FileRecoveryMergeResultV1::Conflicted {
            recovery_revision: 2,
            current_revision: 4,
            current_content_hash: "sha256:current".to_string(),
            disk_changed: true,
            merged_text: "<<<<<<< ours\n=======\n>>>>>>> theirs\n".to_string(),
        };
        assert_eq!(
            serde_json::to_value(clean).expect("clean outcome")["status"],
            "clean"
        );
        assert_eq!(
            serde_json::to_value(conflicted).expect("conflicted outcome")["status"],
            "conflicted"
        );
    }
}
