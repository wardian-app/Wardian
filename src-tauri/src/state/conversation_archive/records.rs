use wardian_core::conversations::{
    ConversationBoundaryReason, ConversationNarrativeRecord, ConversationRecordKind,
    ConversationSourceRecord, ConversationSpeakerType, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

use super::ConversationArchiveContext;

pub fn narrative_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationNarrativeRecord> {
    let kind = record_kind_from_chat_event_kind(&event.kind)?;
    let role = event.role.as_ref().map(role_to_string);

    Some(ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: event.turn_id.clone(),
        at: event
            .created_at
            .clone()
            .unwrap_or_else(current_rfc3339_millis),
        kind,
        role,
        speaker_type: event.role.as_ref().map(speaker_type_from_role),
        text: event.text.clone(),
        tool: tool_name_from_chat_event(event),
        status: event.status.as_ref().map(status_to_string),
        summary: event.title.clone(),
        excerpt: None,
        event_refs: vec![event.id.clone()],
        source_refs: event.source.iter().cloned().collect(),
        artifact_refs: Vec::new(),
    })
}

pub(super) fn source_record_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationSourceRecord> {
    let source_kind = event
        .source
        .clone()
        .or_else(|| metadata_source_kind(&event.provider, &event.metadata))?;

    Some(ConversationSourceRecord {
        schema: CONVERSATION_SCHEMA,
        source_id: format!("src_{seq}"),
        provider: event.provider.clone(),
        provider_session_id: metadata_string(&event.metadata, "opencode_session_id")
            .or_else(|| metadata_string(&event.metadata, "provider_session_id")),
        source_kind,
        source_path: event
            .path
            .clone()
            .or_else(|| metadata_string(&event.metadata, "source_path"))
            .or_else(|| metadata_string(&event.metadata, "path"))
            .or_else(|| metadata_string(&event.metadata, "log_path")),
        cursor: metadata_string(&event.metadata, "cursor")
            .or_else(|| event.sequence.map(|sequence| sequence.to_string())),
        offset: metadata_u64(&event.metadata, "offset"),
        row_id: metadata_string(&event.metadata, "part_id"),
        provider_event_type: metadata_string(&event.metadata, "raw_type")
            .or_else(|| metadata_string(&event.metadata, "provider_type")),
        hash: metadata_string(&event.metadata, "hash"),
        artifact_ref: metadata_string(&event.metadata, "artifact_ref"),
    })
}

pub(super) fn matching_delivered_input_record_index(
    records: &[ConversationNarrativeRecord],
    event: &AgentChatEvent,
) -> Option<usize> {
    if event.kind != AgentChatEventKind::Message
        || event.role.as_ref() != Some(&AgentChatRole::User)
    {
        return None;
    }
    let event_text = event.text.as_deref()?.trim();
    if event_text.is_empty() {
        return None;
    }

    let mut matches = records.iter().enumerate().filter_map(|(index, record)| {
        (record.kind == ConversationRecordKind::Message
            && record.role.as_deref() == Some("user")
            && record.turn_id.is_none()
            && record
                .event_refs
                .iter()
                .all(|event_ref| event_ref.starts_with("generated:"))
            && record
                .text
                .as_deref()
                .is_some_and(|text| text.trim() == event_text))
        .then_some(index)
    });
    let index = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn tool_name_from_chat_event(event: &AgentChatEvent) -> Option<String> {
    metadata_string(&event.metadata, "tool_name")
        .or_else(|| event.title.clone())
        .map(|tool| tool.trim().to_string())
        .filter(|tool| !tool.is_empty())
}

pub(super) fn generated_event_from_record(
    context: &ConversationArchiveContext,
    conversation_id: &str,
    record: &mut ConversationNarrativeRecord,
) -> AgentChatEvent {
    let event_id = format!("generated:{conversation_id}:{}", record.seq);
    if record.event_refs.is_empty() {
        record.event_refs.push(event_id.clone());
    }

    AgentChatEvent {
        id: event_id,
        session_id: context.agent_id.clone(),
        provider: context.provider.clone(),
        kind: match record.kind {
            ConversationRecordKind::Message => AgentChatEventKind::Message,
            ConversationRecordKind::ToolCall => AgentChatEventKind::ToolCall,
            ConversationRecordKind::ToolResult => AgentChatEventKind::ToolResult,
            ConversationRecordKind::Approval => AgentChatEventKind::Approval,
            ConversationRecordKind::Error => AgentChatEventKind::Error,
            ConversationRecordKind::Lifecycle | ConversationRecordKind::Status => {
                AgentChatEventKind::Status
            }
        },
        role: record.role.as_deref().and_then(agent_role_from_record_role),
        text: record.text.clone(),
        title: record.summary.clone(),
        status: (record.kind == ConversationRecordKind::Lifecycle).then_some(AgentChatStatus::Idle),
        turn_id: context.provider_source_key.clone(),
        source: record.source_refs.first().cloned(),
        command: record.tool.clone(),
        exit_code: None,
        path: None,
        language: None,
        created_at: Some(record.at.clone()),
        sequence: Some(record.seq),
        metadata: serde_json::json!({
            "generated": true,
            "conversation_record_kind": record_kind_to_string(record.kind),
        }),
    }
}

pub(super) fn generated_sources_from_record(
    _context: &ConversationArchiveContext,
    record: &mut ConversationNarrativeRecord,
) -> Vec<ConversationSourceRecord> {
    let mut sources = Vec::new();
    for source_ref in record.source_refs.clone() {
        if let Some(sender_agent_id) = source_ref.strip_prefix("agent:").map(ToString::to_string) {
            sources.push(ConversationSourceRecord {
                schema: CONVERSATION_SCHEMA,
                source_id: source_ref,
                provider: "wardian".to_string(),
                provider_session_id: Some(sender_agent_id),
                source_kind: "wardian_agent".to_string(),
                source_path: None,
                cursor: Some(record.seq.to_string()),
                offset: None,
                row_id: None,
                provider_event_type: Some("delivered_input".to_string()),
                hash: None,
                artifact_ref: None,
            });
        }
    }

    if record.kind == ConversationRecordKind::Lifecycle {
        let source_id = format!("src_{}", record.seq);
        if !record.source_refs.iter().any(|source| source == &source_id) {
            record.source_refs.push(source_id.clone());
        }
        sources.push(ConversationSourceRecord {
            schema: CONVERSATION_SCHEMA,
            source_id,
            provider: "wardian".to_string(),
            provider_session_id: None,
            source_kind: "wardian_lifecycle".to_string(),
            source_path: None,
            cursor: Some(record.seq.to_string()),
            offset: None,
            row_id: None,
            provider_event_type: record.status.clone(),
            hash: None,
            artifact_ref: None,
        });
    }

    sources
}

fn agent_role_from_record_role(role: &str) -> Option<AgentChatRole> {
    match role {
        "user" => Some(AgentChatRole::User),
        "assistant" => Some(AgentChatRole::Assistant),
        "system" => Some(AgentChatRole::System),
        "tool" => Some(AgentChatRole::Tool),
        _ => None,
    }
}

fn record_kind_to_string(kind: ConversationRecordKind) -> &'static str {
    match kind {
        ConversationRecordKind::Message => "message",
        ConversationRecordKind::ToolCall => "tool_call",
        ConversationRecordKind::ToolResult => "tool_result",
        ConversationRecordKind::Approval => "approval",
        ConversationRecordKind::Error => "error",
        ConversationRecordKind::Lifecycle => "lifecycle",
        ConversationRecordKind::Status => "status",
    }
}

pub fn narrative_from_delivered_input(
    at: &str,
    text: &str,
    sender_agent_id: Option<&str>,
    seq: u64,
) -> ConversationNarrativeRecord {
    ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: None,
        at: at.to_string(),
        kind: ConversationRecordKind::Message,
        role: Some("user".to_string()),
        speaker_type: Some(if sender_agent_id.is_some() {
            ConversationSpeakerType::Agent
        } else {
            ConversationSpeakerType::Unknown
        }),
        text: Some(text.to_string()),
        tool: None,
        status: None,
        summary: None,
        excerpt: None,
        event_refs: Vec::new(),
        source_refs: sender_agent_id
            .map(|sender| vec![format!("agent:{sender}")])
            .unwrap_or_default(),
        artifact_refs: Vec::new(),
    }
}

pub fn lifecycle_record(
    seq: u64,
    reason: ConversationBoundaryReason,
    at: &str,
) -> ConversationNarrativeRecord {
    ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: None,
        at: at.to_string(),
        kind: ConversationRecordKind::Lifecycle,
        role: Some("system".to_string()),
        speaker_type: Some(ConversationSpeakerType::System),
        text: None,
        tool: None,
        status: Some(boundary_reason_to_string(reason)),
        summary: Some(format!(
            "conversation {}",
            boundary_reason_to_string(reason)
        )),
        excerpt: None,
        event_refs: Vec::new(),
        source_refs: Vec::new(),
        artifact_refs: Vec::new(),
    }
}

pub(super) fn record_kind_from_chat_event_kind(
    kind: &AgentChatEventKind,
) -> Option<ConversationRecordKind> {
    match kind {
        AgentChatEventKind::Message => Some(ConversationRecordKind::Message),
        AgentChatEventKind::ToolCall => Some(ConversationRecordKind::ToolCall),
        AgentChatEventKind::ToolResult => Some(ConversationRecordKind::ToolResult),
        AgentChatEventKind::Approval => Some(ConversationRecordKind::Approval),
        AgentChatEventKind::Status => None,
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

fn has_provider_metadata(metadata: &serde_json::Value) -> bool {
    [
        "opencode_session_id",
        "part_id",
        "raw_type",
        "provider_type",
        "cursor",
        "offset",
        "provider_log",
        "provider_source",
        "log_source",
        "source_path",
        "log_path",
        "hash",
        "artifact_ref",
    ]
    .iter()
    .any(|key| metadata.get(*key).is_some())
}

fn metadata_source_kind(provider: &str, metadata: &serde_json::Value) -> Option<String> {
    metadata_string(metadata, "log_source")
        .or_else(|| metadata_string(metadata, "provider_source"))
        .or_else(|| metadata_string(metadata, "provider_type"))
        .or_else(|| metadata_string(metadata, "raw_type"))
        .or_else(|| has_provider_metadata(metadata).then(|| format!("{provider}_metadata")))
}

pub(super) fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    let value = metadata.get(key)?;
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn metadata_u64(metadata: &serde_json::Value, key: &str) -> Option<u64> {
    let value = metadata.get(key)?;
    value.as_u64().or_else(|| {
        value
            .as_i64()
            .and_then(|number| u64::try_from(number).ok())
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

fn boundary_reason_to_string(reason: ConversationBoundaryReason) -> String {
    match reason {
        ConversationBoundaryReason::Spawn => "spawn",
        ConversationBoundaryReason::ProviderSourceChanged => "provider_source_changed",
        ConversationBoundaryReason::Clear => "clear",
        ConversationBoundaryReason::WorktreeSwitch => "worktree_switch",
        ConversationBoundaryReason::LoggingEnabled => "logging_enabled",
        ConversationBoundaryReason::Shutdown => "shutdown",
    }
    .to_string()
}

pub(super) fn current_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
