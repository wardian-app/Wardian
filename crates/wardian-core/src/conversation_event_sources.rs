//! Shared deterministic source DTO constructors for capture and archive validation.
//!
//! These functions use only supplied events or records. They perform no I/O,
//! consult no archive ownership, and do not allocate or plan narrative records.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::conversations::{
    ConversationNarrativeRecord, ConversationRecordKind, ConversationSourceRecord,
    CONVERSATION_SCHEMA,
};
use crate::models::chat::{AgentChatEvent, AgentChatEventKind};

/// Derive the existing comprehensive source shape from an event and narrative sequence.
///
/// Coordinate-based Antigravity provenance takes precedence over the fallback.
/// This pure mapping lets capture and validation use the same source identity rules.
pub fn source_record_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationSourceRecord> {
    antigravity_source_record_from_chat_event(event)
        .or_else(|| fallback_source_record_from_chat_event(event, seq))
}

/// Derive the legacy fallback source shape without reading provider or archive state.
///
/// This preserves sender, watch and receipted-observation identities so callers
/// comparing previously published fallback rows use the original construction rules.
pub fn fallback_source_record_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationSourceRecord> {
    if let Some(sender_agent_id) = metadata_string(&event.metadata, "sender_agent_id") {
        return Some(ConversationSourceRecord {
            schema: CONVERSATION_SCHEMA,
            source_id: format!("agent:{sender_agent_id}"),
            provider: "wardian".to_string(),
            provider_session_id: Some(sender_agent_id),
            source_kind: "wardian_agent".to_string(),
            source_path: None,
            cursor: Some(seq.to_string()),
            offset: None,
            row_id: None,
            provider_event_type: Some("delivered_input".to_string()),
            hash: None,
            artifact_ref: None,
        });
    }
    let source_kind = event
        .source
        .clone()
        .or_else(|| metadata_source_kind(&event.provider, &event.metadata))?;
    // Bound watch projections have no native log position. Keep their source
    // distinct from a later captured native row owning the same narrative.
    let native_watch = event.metadata["provider_log"] == true
        && event
            .id
            .starts_with(&format!("{}:watch_native:", event.session_id))
        && event.metadata.get("transcript_cursor").is_some();

    let receipted_mock =
        event.provider == "mock" && event.metadata["mock_observation_id"].is_string();
    let source_id = if native_watch {
        format!("watch:{}", event.id)
    } else if (event.provider.eq_ignore_ascii_case("codex")
        && event.metadata["provider_log"] == true)
        || receipted_mock
    {
        let digest = Sha256::digest(event.id.as_bytes());
        format!(
            "src_{}_observation_{}",
            if receipted_mock { "mock" } else { "codex" },
            digest
                .iter()
                .take(12)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join("")
        )
    } else {
        format!("src_{seq}")
    };
    Some(ConversationSourceRecord {
        schema: CONVERSATION_SCHEMA,
        source_id,
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
        cursor: if native_watch {
            None
        } else if receipted_mock {
            // The producer receipt is stable when the live display is renumbered.
            // A framed native row retains its actual physical coordinate instead.
            metadata_string(&event.metadata, "chat_source_ref")
                .or_else(|| metadata_string(&event.metadata, "mock_observation_id"))
        } else {
            metadata_string(&event.metadata, "cursor")
                .or_else(|| event.sequence.map(|sequence| sequence.to_string()))
        },
        offset: metadata_u64(&event.metadata, "offset"),
        row_id: metadata_string(&event.metadata, "part_id"),
        provider_event_type: metadata_string(&event.metadata, "raw_type")
            .or_else(|| metadata_string(&event.metadata, "provider_type")),
        hash: metadata_string(&event.metadata, "hash"),
        artifact_ref: metadata_string(&event.metadata, "artifact_ref"),
    })
}

/// Derive Antigravity provenance from a native database coordinate only.
///
/// The identity remains independent of display order and evolving payloads.
/// This pure constructor returns `None` for ineligible events without a fallback.
pub fn antigravity_source_record_from_chat_event(
    event: &AgentChatEvent,
) -> Option<ConversationSourceRecord> {
    if !event.provider.eq_ignore_ascii_case("antigravity")
        || event.source.as_deref() != Some("conversation_database")
        || event.metadata["provider_log"] != true
        || metadata_string(&event.metadata, "sender_agent_id").is_some()
        || (event
            .id
            .starts_with(&format!("{}:watch_native:", event.session_id))
            && event.metadata.get("transcript_cursor").is_some())
    {
        return None;
    }
    let path = metadata_string(&event.metadata, "log_path")?;
    let database = Path::new(&path);
    if !database.extension()?.to_str()?.eq_ignore_ascii_case("db") {
        return None;
    }
    let step = event.metadata["step_index"].as_u64()?.to_string();
    let (kind, ordinal) = match event.kind {
        AgentChatEventKind::Message => ("message", String::new()),
        AgentChatEventKind::ToolCall => (
            "tool_call",
            event.metadata["tool_ordinal"].as_u64()?.to_string(),
        ),
        AgentChatEventKind::ToolResult => (
            "tool_result",
            event.metadata["tool_ordinal"].as_u64()?.to_string(),
        ),
        _ => return None,
    };
    let mut identity = Sha256::new();
    for part in ["antigravity_database_v1", &path, &step, kind, &ordinal] {
        identity.update(part.as_bytes());
        identity.update(b"\0");
    }
    Some(ConversationSourceRecord {
        schema: CONVERSATION_SCHEMA,
        source_id: format!("src_antigravity_observation_{:x}", identity.finalize()),
        provider: event.provider.clone(),
        provider_session_id: metadata_string(&event.metadata, "provider_session_id")
            .or_else(|| database.file_stem()?.to_str().map(str::to_string)),
        source_kind: "conversation_database".to_string(),
        source_path: Some(path),
        cursor: Some(step),
        offset: None,
        row_id: None,
        provider_event_type: Some(kind.to_string()),
        hash: None,
        artifact_ref: None,
    })
}

/// Derive sender and lifecycle source DTOs for an already constructed narrative record.
///
/// This performs no I/O. Lifecycle records retain the existing local mutation:
/// append their source reference to the supplied record only when it is absent.
pub fn generated_sources_from_record(
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

fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
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
