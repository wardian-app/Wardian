//! Public model types for agent-owned conversation archives.

use serde::{Deserialize, Serialize};

pub const CONVERSATION_SCHEMA: u8 = 1;
pub const CONVERSATION_INLINE_TEXT_LIMIT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLoggingSetting {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentConversationLoggingSetting {
    Default,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStatus {
    Open,
    Closed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationBoundaryReason {
    Spawn,
    ProviderSourceChanged,
    Clear,
    WorktreeSwitch,
    LoggingEnabled,
    Shutdown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationRecordKind {
    Message,
    ToolCall,
    ToolResult,
    Approval,
    Error,
    Lifecycle,
    Status,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSpeakerType {
    User,
    Assistant,
    Agent,
    Tool,
    System,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationFormatVersions {
    pub manifest: u8,
    pub conversation: u8,
    pub events: u8,
    pub sources: u8,
}

impl Default for ConversationFormatVersions {
    fn default() -> Self {
        Self {
            manifest: 1,
            conversation: 1,
            events: 1,
            sources: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationManifest {
    pub schema: u8,
    pub conversation_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub agent_class: String,
    pub workspace: String,
    pub provider: String,
    pub provider_session_ids: Vec<String>,
    pub effective_logging: ConversationLoggingSetting,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub status: ConversationStatus,
    pub boundary_reason: ConversationBoundaryReason,
    pub format_versions: ConversationFormatVersions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationNarrativeRecord {
    pub schema: u8,
    pub seq: u64,
    pub at: String,
    pub kind: ConversationRecordKind,
    pub role: Option<String>,
    pub speaker_type: Option<ConversationSpeakerType>,
    pub text: Option<String>,
    pub tool: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub excerpt: Option<String>,
    pub event_refs: Vec<String>,
    pub source_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_serializes_archive_enums_as_snake_case() {
        let manifest = ConversationManifest {
            schema: CONVERSATION_SCHEMA,
            conversation_id: "conv_20260615_000000_agent_1".to_string(),
            agent_id: "agent-1".to_string(),
            agent_name: "Coder One".to_string(),
            agent_class: "coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["session-1".to_string()],
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-06-15T00:00:00.000Z".to_string(),
            updated_at: "2026-06-15T00:01:00.000Z".to_string(),
            closed_at: None,
            status: ConversationStatus::Interrupted,
            boundary_reason: ConversationBoundaryReason::ProviderSourceChanged,
            format_versions: ConversationFormatVersions::default(),
        };

        let json = serde_json::to_value(&manifest).unwrap();

        assert_eq!(json["schema"], CONVERSATION_SCHEMA);
        assert_eq!(json["status"], "interrupted");
        assert_eq!(json["boundary_reason"], "provider_source_changed");
        assert_eq!(json["effective_logging"], "enabled");
    }

    #[test]
    fn narrative_tool_result_can_reference_artifacts() {
        let record = ConversationNarrativeRecord {
            schema: CONVERSATION_SCHEMA,
            seq: 7,
            at: "2026-06-15T00:00:07.000Z".to_string(),
            kind: ConversationRecordKind::ToolResult,
            role: Some("tool".to_string()),
            speaker_type: Some(ConversationSpeakerType::Tool),
            text: None,
            tool: Some("shell_command".to_string()),
            status: Some("success".to_string()),
            summary: Some("captured long command output".to_string()),
            excerpt: Some("first 8 KiB of output".to_string()),
            event_refs: vec!["events:7".to_string()],
            source_refs: vec!["sources:3".to_string()],
            artifact_refs: vec!["artifacts/tool-result-7.txt".to_string()],
        };

        let json = serde_json::to_value(&record).unwrap();

        assert_eq!(json["kind"], "tool_result");
        assert_eq!(json["speaker_type"], "tool");
        assert_eq!(json["tool"], "shell_command");
        assert_eq!(json["artifact_refs"][0], "artifacts/tool-result-7.txt");
    }
}
