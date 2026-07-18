use serde::{Deserialize, Serialize};

pub const ARTIFACT_SCHEMA_V1: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactOriginV1 {
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactVersionV1 {
    pub version_id: String,
    pub sequence: u64,
    pub content_hash: String,
    pub size_bytes: u64,
    pub presented_at_ms: u64,
    #[serde(default)]
    pub addressed_comment_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactReviewStatus {
    Presented,
    FeedbackSent,
    Updated,
    Approved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifestV1 {
    pub schema: u8,
    pub artifact_id: String,
    pub canonical_path: String,
    pub title: String,
    pub description: Option<String>,
    pub origin: ArtifactOriginV1,
    pub status: ArtifactReviewStatus,
    pub active: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub versions: Vec<ArtifactVersionV1>,
    pub latest_review_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexEntryV1 {
    pub artifact_id: String,
    pub canonical_path: String,
    pub title: String,
    pub origin_session_id: String,
    pub origin_agent_name: String,
    pub latest_version_id: String,
    pub status: ArtifactReviewStatus,
    pub active: bool,
    pub attention: bool,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexV1 {
    pub schema: u8,
    pub entries: Vec<ArtifactIndexEntryV1>,
}

impl Default for ArtifactIndexV1 {
    fn default() -> Self {
        Self {
            schema: ARTIFACT_SCHEMA_V1,
            entries: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_schema_round_trips_and_statuses_are_snake_case() {
        let manifest = ArtifactManifestV1 {
            schema: ARTIFACT_SCHEMA_V1,
            artifact_id: "artifact-1".into(),
            canonical_path: "/workspace/report.md".into(),
            title: "Report".into(),
            description: Some("Review this".into()),
            origin: ArtifactOriginV1 {
                session_id: "session-1".into(),
                agent_id: "agent-1".into(),
                agent_name: "Writer".into(),
                provider: "codex".into(),
            },
            status: ArtifactReviewStatus::FeedbackSent,
            active: true,
            created_at_ms: 1,
            updated_at_ms: 2,
            versions: vec![ArtifactVersionV1 {
                version_id: "version-1".into(),
                sequence: 1,
                content_hash: format!("sha256:{}", "a".repeat(64)),
                size_bytes: 4,
                presented_at_ms: 2,
                addressed_comment_ids: vec!["comment-1".into()],
            }],
            latest_review_id: None,
        };

        let json = serde_json::to_value(&manifest).expect("serialize");
        assert_eq!(json["status"], "feedback_sent");
        assert_eq!(
            serde_json::from_value::<ArtifactManifestV1>(json).expect("deserialize"),
            manifest
        );
    }

    #[test]
    fn artifact_schema_rejects_unknown_fields() {
        let value = serde_json::json!({
            "schema": 1,
            "entries": [],
            "unexpected": true,
        });
        assert!(serde_json::from_value::<ArtifactIndexV1>(value).is_err());
    }
}
