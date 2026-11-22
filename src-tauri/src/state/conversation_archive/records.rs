use std::io;

use wardian_core::conversations::{
    ConversationBoundaryReason, ConversationInputOrigin, ConversationNarrativeRecord,
    ConversationRecordKind, ConversationSourceRecord, ConversationSpeakerType, CONVERSATION_SCHEMA,
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
    let input_origin = input_origin_from_chat_event(event);
    let input_purpose = metadata_string(&event.metadata, "input_purpose").or_else(|| {
        input_origin.as_ref().map(|origin| match origin {
            ConversationInputOrigin::HumanInput | ConversationInputOrigin::AgentInput => {
                "request".to_string()
            }
            ConversationInputOrigin::ContextInjection => "context".to_string(),
            ConversationInputOrigin::ProviderInternal => "internal".to_string(),
        })
    });

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
        speaker_type: event.role.as_ref().map(|role| {
            if role == &AgentChatRole::User
                && metadata_string(&event.metadata, "sender_agent_id").is_some()
            {
                ConversationSpeakerType::Agent
            } else {
                speaker_type_from_role(role)
            }
        }),
        input_origin,
        input_purpose,
        request_root_id: metadata_string(&event.metadata, "request_root_id").or_else(|| {
            matches!(
                input_origin,
                Some(ConversationInputOrigin::HumanInput | ConversationInputOrigin::AgentInput)
            )
            .then(|| event.turn_id.clone())
            .flatten()
        }),
        causal_ref: metadata_string(&event.metadata, "causal_ref"),
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
    wardian_core::conversation_event_sources::source_record_from_chat_event(event, seq)
}

pub(super) fn fallback_source_record_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationSourceRecord> {
    wardian_core::conversation_event_sources::fallback_source_record_from_chat_event(event, seq)
}

/// Retain an unbound watch observation's published fallback cursor on retry.
///
/// Merging provider events renumbers the live presentation sequence. The
/// uniquely matched durable observation keeps the ordinal first published,
/// including older archives where it differs from the narrative sequence.
/// Observations with native source evidence retain the strict cursor check.
/// Explicit cursors and every other provenance field still come from `event`.
pub(super) fn source_record_for_retry(
    event: &AgentChatEvent,
    seq: u64,
    archived_event: Option<&AgentChatEvent>,
) -> Option<ConversationSourceRecord> {
    let mut source = source_record_from_chat_event(event, seq)?;
    let Some(archived) = archived_event else {
        return Some(source);
    };
    if source.source_id.starts_with("src_")
        && event.kind == AgentChatEventKind::Message
        && archived.id == event.id
        && archived.session_id == event.session_id
        && archived.provider == event.provider
        && archived.kind == event.kind
        && archived.source == event.source
        && metadata_string(&event.metadata, "transcript_cursor").is_some()
        && metadata_string(&archived.metadata, "transcript_cursor").is_some()
        && event.metadata.get("cursor").is_none()
        && archived.metadata.get("cursor").is_none()
        && event.metadata["provider_log"] != true
        && archived.metadata["provider_log"] != true
        && source.source_path.is_none()
        && source.offset.is_none()
        && source.row_id.is_none()
        && source.provider_event_type.is_none()
        && source.hash.is_none()
        && source.artifact_ref.is_none()
    {
        source.cursor = archived.sequence.map(|sequence| sequence.to_string());
    }
    Some(source)
}

pub(super) fn matching_delivered_input_record_index(
    records: &[ConversationNarrativeRecord],
    event: &AgentChatEvent,
) -> io::Result<Option<usize>> {
    // A physical native observation does not prove correspondence with an
    // accepted generated input. Keep these identities independent.
    if event.metadata["chat_source_ref"].is_string() {
        return Ok(None);
    }
    if event.kind != AgentChatEventKind::Message
        || event.role.as_ref() != Some(&AgentChatRole::User)
    {
        return Ok(None);
    }
    if matches!(
        metadata_string(&event.metadata, "input_origin").as_deref(),
        Some("provider_internal" | "context_injection")
    ) {
        return Ok(None);
    }
    let Some(event_text) = event.text.as_deref().map(str::trim) else {
        return Ok(None);
    };
    if event_text.is_empty() {
        return Ok(None);
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
    let Some(index) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Ok(None);
    }
    Ok(Some(index))
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
            "input_origin": record.input_origin.map(input_origin_to_string),
            "input_purpose": record.input_purpose.clone(),
            "request_root_id": record.request_root_id.clone(),
            "causal_ref": record.causal_ref.clone(),
            "archive_record": record,
        }),
    }
}

pub(super) fn generated_sources_from_record(
    _context: &ConversationArchiveContext,
    record: &mut ConversationNarrativeRecord,
) -> Vec<ConversationSourceRecord> {
    wardian_core::conversation_event_sources::generated_sources_from_record(record)
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
        input_origin: Some(if sender_agent_id.is_some() {
            ConversationInputOrigin::AgentInput
        } else {
            ConversationInputOrigin::HumanInput
        }),
        input_purpose: Some("request".to_string()),
        request_root_id: Some(format!("wardian:input:{seq}")),
        causal_ref: sender_agent_id.map(|sender| format!("wardian:agent:{sender}")),
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
        input_origin: None,
        input_purpose: None,
        request_root_id: None,
        causal_ref: None,
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

pub(super) fn input_origin_from_chat_event(
    event: &AgentChatEvent,
) -> Option<ConversationInputOrigin> {
    metadata_string(&event.metadata, "input_origin")
        .and_then(|value| parse_input_origin(&value))
        .or_else(|| {
            if event.kind != AgentChatEventKind::Message {
                return None;
            }
            match event.role.as_ref() {
                Some(AgentChatRole::User)
                    if metadata_string(&event.metadata, "sender_agent_id").is_some() =>
                {
                    Some(ConversationInputOrigin::AgentInput)
                }
                Some(AgentChatRole::User) => Some(ConversationInputOrigin::HumanInput),
                Some(AgentChatRole::System) => Some(ConversationInputOrigin::ProviderInternal),
                _ => None,
            }
        })
}

fn parse_input_origin(value: &str) -> Option<ConversationInputOrigin> {
    match value.trim() {
        "human_input" => Some(ConversationInputOrigin::HumanInput),
        "agent_input" => Some(ConversationInputOrigin::AgentInput),
        "context_injection" => Some(ConversationInputOrigin::ContextInjection),
        "provider_internal" => Some(ConversationInputOrigin::ProviderInternal),
        _ => None,
    }
}

fn input_origin_to_string(origin: ConversationInputOrigin) -> &'static str {
    match origin {
        ConversationInputOrigin::HumanInput => "human_input",
        ConversationInputOrigin::AgentInput => "agent_input",
        ConversationInputOrigin::ContextInjection => "context_injection",
        ConversationInputOrigin::ProviderInternal => "provider_internal",
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
        // Memory has its own durable store and retention lifecycle.
        AgentChatEventKind::Memory => None,
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
