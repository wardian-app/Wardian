//! SQLite source identity is a database coordinate, independent of display order.

use std::{io, path::Path};

use sha2::{Digest, Sha256};
use wardian_core::conversations::{ConversationSourceRecord, CONVERSATION_SCHEMA};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

use super::records::{fallback_source_record_from_chat_event, metadata_string};

/// Creates stable provenance for each message or wire-ordered tool slot. Payloads
/// can evolve, so neither content hashes nor tool arguments identify the source.
pub(super) fn source_record(event: &AgentChatEvent) -> Option<ConversationSourceRecord> {
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

/// Reuses a published ordinal-based row only for its exact archived observation
/// at the same native coordinate. The historical row is retained byte-for-byte;
/// its fallback cursor is never interpreted as a provider step or upgraded.
pub(super) fn legacy_source_for_retry(
    event: &AgentChatEvent,
    archived: Option<&AgentChatEvent>,
    seq: u64,
    published: &ConversationSourceRecord,
) -> io::Result<ConversationSourceRecord> {
    let conflict = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "conflicting legacy Antigravity source identity: {}",
                published.source_id
            ),
        )
    };
    let archived = archived.ok_or_else(conflict)?;
    let current_native = source_record(event).ok_or_else(conflict)?;
    let archived_native = source_record(archived).ok_or_else(conflict)?;
    if event.id != archived.id
        || event.session_id != archived.session_id
        || current_native != archived_native
        || event.metadata.get("cursor").is_some()
        || archived.metadata.get("cursor").is_some()
        || published
            .cursor
            .as_deref()
            .and_then(|cursor| cursor.parse::<u64>().ok())
            .is_none()
    {
        return Err(conflict());
    }
    let database_session = current_native
        .source_path
        .as_deref()
        .and_then(|path| Path::new(path).file_stem()?.to_str());
    if current_native.provider_session_id.as_deref() != database_session {
        return Err(conflict());
    }
    let mut expected =
        fallback_source_record_from_chat_event(archived, seq).ok_or_else(conflict)?;
    // Older events can have gained this proven session after the source row was
    // published. Missing historical metadata does not require rewriting history.
    if published.provider_session_id.is_none() {
        expected.provider_session_id = None;
    }
    expected.cursor = published.cursor.clone();
    // These fields described the original payload, including a tool target in
    // source_path. Enrichment can persist newer payload metadata on the event;
    // the proven native owner still retains its historical source observation.
    expected.source_path = published.source_path.clone();
    expected.provider_event_type = published.provider_event_type.clone();
    expected.hash = published.hash.clone();
    if expected != *published {
        return Err(conflict());
    }
    Ok(published.clone())
}
