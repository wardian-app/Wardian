//! SQLite source identity is a database coordinate, independent of display order.

use std::{io, path::Path};

use wardian_core::conversations::ConversationSourceRecord;
use wardian_core::models::chat::AgentChatEvent;

use super::records::fallback_source_record_from_chat_event;

/// Creates stable provenance for each message or wire-ordered tool slot. Payloads
/// can evolve, so neither content hashes nor tool arguments identify the source.
pub(super) fn source_record(event: &AgentChatEvent) -> Option<ConversationSourceRecord> {
    wardian_core::conversation_event_sources::antigravity_source_record_from_chat_event(event)
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
