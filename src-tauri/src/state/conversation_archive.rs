use std::{collections::HashMap, sync::Mutex};

use wardian_core::conversations::{
    ConversationNarrativeRecord, ConversationRecordKind, ConversationSpeakerType,
    CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

#[derive(Debug, Default)]
pub struct ConversationArchiveState {
    #[allow(dead_code)]
    active: Mutex<HashMap<String, ActiveConversationHandle>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConversationHandle {
    pub conversation_id: String,
    pub next_seq: u64,
}

pub fn narrative_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationNarrativeRecord> {
    let kind = record_kind_from_chat_event_kind(&event.kind)?;
    let role = event.role.as_ref().map(role_to_string);

    Some(ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        at: event
            .created_at
            .clone()
            .unwrap_or_else(current_rfc3339_millis),
        kind,
        role,
        speaker_type: event.role.as_ref().map(speaker_type_from_role),
        text: event.text.clone(),
        tool: event.title.clone().or_else(|| event.command.clone()),
        status: event.status.as_ref().map(|status| status_to_string(status)),
        summary: event.title.clone(),
        excerpt: None,
        event_refs: vec![event.id.clone()],
        source_refs: event.source.iter().cloned().collect(),
        artifact_refs: Vec::new(),
    })
}

fn record_kind_from_chat_event_kind(kind: &AgentChatEventKind) -> Option<ConversationRecordKind> {
    match kind {
        AgentChatEventKind::Message => Some(ConversationRecordKind::Message),
        AgentChatEventKind::ToolCall => Some(ConversationRecordKind::ToolCall),
        AgentChatEventKind::ToolResult => Some(ConversationRecordKind::ToolResult),
        AgentChatEventKind::Approval => Some(ConversationRecordKind::Approval),
        AgentChatEventKind::Status => Some(ConversationRecordKind::Status),
        AgentChatEventKind::Error => Some(ConversationRecordKind::Error),
        AgentChatEventKind::TerminalOutput => None,
    }
}

fn role_to_string(role: &AgentChatRole) -> String {
    match role {
        AgentChatRole::User => "user",
        AgentChatRole::Assistant => "assistant",
        AgentChatRole::System => "system",
        AgentChatRole::Tool => "tool",
    }
    .to_string()
}

fn speaker_type_from_role(role: &AgentChatRole) -> ConversationSpeakerType {
    match role {
        AgentChatRole::User => ConversationSpeakerType::User,
        AgentChatRole::Assistant => ConversationSpeakerType::Assistant,
        AgentChatRole::System => ConversationSpeakerType::System,
        AgentChatRole::Tool => ConversationSpeakerType::Tool,
    }
}

fn status_to_string(status: &wardian_core::models::chat::AgentChatStatus) -> String {
    match status {
        wardian_core::models::chat::AgentChatStatus::Running => "running",
        wardian_core::models::chat::AgentChatStatus::Succeeded => "succeeded",
        wardian_core::models::chat::AgentChatStatus::Failed => "failed",
        wardian_core::models::chat::AgentChatStatus::ActionRequired => "action_required",
        wardian_core::models::chat::AgentChatStatus::Cancelled => "cancelled",
        wardian_core::models::chat::AgentChatStatus::Idle => "idle",
        wardian_core::models::chat::AgentChatStatus::Processing => "processing",
        wardian_core::models::chat::AgentChatStatus::Unknown => "unknown",
    }
    .to_string()
}

fn current_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::narrative_from_chat_event;
    use wardian_core::conversations::{
        ConversationRecordKind, ConversationSpeakerType, CONVERSATION_SCHEMA,
    };
    use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

    #[test]
    fn user_chat_message_converts_to_primary_narrative_record() {
        let event = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::User),
            Some("Please inspect the workspace."),
        );

        let record = narrative_from_chat_event(&event, 42).expect("narrative record");

        assert_eq!(record.schema, CONVERSATION_SCHEMA);
        assert_eq!(record.seq, 42);
        assert_eq!(record.at, "2026-06-15T00:00:00.000Z");
        assert_eq!(record.kind, ConversationRecordKind::Message);
        assert_eq!(record.role.as_deref(), Some("user"));
        assert_eq!(record.speaker_type, Some(ConversationSpeakerType::User));
        assert_eq!(
            record.text.as_deref(),
            Some("Please inspect the workspace.")
        );
        assert_eq!(record.event_refs, vec!["event-1".to_string()]);
    }

    #[test]
    fn terminal_output_is_not_primary_narrative() {
        let event = chat_event(
            "event-terminal",
            AgentChatEventKind::TerminalOutput,
            None,
            Some("raw terminal output"),
        );

        assert!(narrative_from_chat_event(&event, 1).is_none());
    }

    fn chat_event(
        id: &str,
        kind: AgentChatEventKind,
        role: Option<AgentChatRole>,
        text: Option<&str>,
    ) -> AgentChatEvent {
        AgentChatEvent {
            id: id.to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind,
            role,
            text: text.map(ToString::to_string),
            title: None,
            status: None,
            turn_id: None,
            source: None,
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: Some("2026-06-15T00:00:00.000Z".to_string()),
            sequence: None,
            metadata: serde_json::json!({}),
        }
    }
}
