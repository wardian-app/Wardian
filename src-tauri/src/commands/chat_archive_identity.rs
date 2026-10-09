//! Compatibility identities proven by a complete native log observation.
use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

use crate::providers::chat_transcript::{
    PROVIDER_EVENT_ID_METADATA_KEY, PROVIDER_LOG_ROW_OFFSET_METADATA_KEY,
};
use sha2::{Digest, Sha256};

/// Share the archive's source-bound completion identity with the live Chat
/// projection so ambiguous or mismatched provider observations stay separate.
pub(super) fn is_codex_stream_completion_pair(a: &AgentChatEvent, b: &AgentChatEvent) -> bool {
    a.role == Some(wardian_core::models::chat::AgentChatRole::Assistant)
        && crate::providers::chat_transcript::codex_display_pair(a, b)
}

/// Preserve the published pre-coordinate ID algorithm as private evidence.
/// The caller still assigns the physical coordinate ID and removes arbitrary
/// legacy aliases. Sequence qualification belongs to the acquisition owner.
pub(crate) fn capture_legacy_identity(event: &mut AgentChatEvent, path: &Path, raw: Option<&str>) {
    if !matches!(event.provider.as_str(), "codex" | "pi")
        || event.kind != AgentChatEventKind::Message
    {
        return;
    }
    let mut original = event.clone();
    if event.provider == "pi" {
        let Some(row) = raw.and_then(|raw| serde_json::from_str::<Value>(raw).ok()) else {
            return;
        };
        if row["type"] != "message" || row["id"].as_str().is_none_or(str::is_empty) {
            return;
        }
        original.turn_id = crate::providers::chat_transcript::pi_original_legacy_turn_id(&row);
    }
    event.metadata["chat_compatibility_legacy_id"] =
        serde_json::json!(legacy_provider_log_event_id(&original, path));
    if let Some(sequence) = event.sequence {
        event.metadata["chat_compatibility_source_sequence"] = serde_json::json!(sequence);
    }
}

/// Observe the native session header without loading its transcript.
pub(crate) fn native_log_session(path: &Path, provider: &str) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(4096).read_to_end(&mut bytes).ok()?;
    let end = bytes.iter().position(|byte| *byte == b'\n')?;
    let header: Value = serde_json::from_slice(&bytes[..end]).ok()?;
    let id = match provider {
        "codex" if header["type"] == "session_meta" => header["payload"]["id"].as_str(),
        "pi" if header["type"] == "session" => header["id"].as_str(),
        _ => None,
    }?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// Pi's pre-envelope-ID projection omitted the native entry ID. Recompute its
/// exact old identity only when a complete session maps it to ONE native entry.
/// Repeated equal prompts and bounded tails cannot establish that bridge.
/// This reads adapter output; it never assigns a turn ID or request root.
pub(crate) fn attach_native_legacy_aliases(
    events: &mut [AgentChatEvent],
    path: &Path,
    content: &str,
    complete: bool,
) {
    if !complete || !events.iter().any(|e| e.provider == "pi") {
        return;
    }
    let Ok(rows) = content
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return;
    };
    let Some(header) = rows.first().filter(|row| row["type"] == "session") else {
        return;
    };
    let Some(session) = header["id"].as_str().filter(|id| !id.is_empty()) else {
        return;
    };
    // Pi owns UUID-named session files; a reused generic path is not a
    // sufficient binding for upgrading an identity-less historical row.
    if !path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with(session))
    {
        return;
    }
    let mut candidates = Vec::new();
    let mut counts = HashMap::<String, usize>::new();
    for (index, event) in events.iter().enumerate() {
        if event.provider != "pi"
            || event.kind != AgentChatEventKind::Message
            || event.source.as_deref() != Some("message")
        {
            continue;
        }
        let Some(row) = event
            .sequence
            .and_then(|seq| seq.checked_sub(1))
            .and_then(|i| rows.get(i as usize))
        else {
            continue;
        };
        let Some(native_id) = row["id"].as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        if row["type"] != "message" || row["message"].get("id").is_some() {
            continue;
        }
        // Count *all* candidate native observations, including any that the
        // installed adapter does not yet root. Never infer an adapter mapping.
        let mut legacy = event.clone();
        legacy.turn_id = None;
        let old_id = legacy_provider_log_event_id(&legacy, path);
        *counts.entry(old_id.clone()).or_default() += 1;
        if event.turn_id.as_deref() == Some(native_id) {
            candidates.push((index, old_id));
        }
    }
    for (index, old_id) in candidates {
        let event = &mut events[index];
        if counts[&old_id] == 1 && old_id != event.id {
            event.metadata["legacy_event_ids"] = serde_json::json!([old_id]);
        }
    }
}

/// Bridge pre-row-offset field identities only when a complete source snapshot
/// proves that the legacy hash belongs to one eligible provider-log row.
/// Claude uses raw-line identities and Codex user mirrors have occurrence-aware
/// identities, so neither can use this field-hash bridge.
pub(crate) fn attach_unique_legacy_row_aliases(
    events: &mut [AgentChatEvent],
    path: &Path,
    complete: bool,
) {
    if !complete {
        return;
    }
    let log_path = path.to_string_lossy();
    let source_row = |event: &AgentChatEvent| {
        event.metadata["provider_log"] == true
            && event.metadata["log_path"].as_str() == Some(log_path.as_ref())
            && !event.provider.eq_ignore_ascii_case("claude")
            && event
                .metadata
                .get(PROVIDER_LOG_ROW_OFFSET_METADATA_KEY)
                .and_then(Value::as_u64)
                .is_some()
    };
    let eligible =
        |event: &AgentChatEvent| source_row(event) && requires_provider_log_row_identity(event);

    let mut counts = HashMap::<String, usize>::new();
    for event in events.iter().filter(|event| source_row(event)) {
        *counts
            .entry(legacy_provider_log_event_id(event, path))
            .or_default() += 1;
    }

    for event in events.iter_mut().filter(|event| eligible(event)) {
        let legacy_id = legacy_provider_log_event_id(event, path);
        if legacy_id == event.id || counts.get(&legacy_id) != Some(&1) {
            continue;
        }
        let Some(metadata) = event.metadata.as_object_mut() else {
            continue;
        };
        let aliases = metadata
            .entry("legacy_event_ids")
            .or_insert_with(|| serde_json::json!([]));
        let Some(aliases) = aliases.as_array_mut() else {
            continue;
        };
        if !aliases
            .iter()
            .any(|alias| alias.as_str() == Some(legacy_id.as_str()))
        {
            aliases.push(serde_json::json!(legacy_id));
        }
    }
}

/// Bridge a legacy field ID only when the open archive and this retry agree on
/// one source row sequence. This covers bounded tails that cannot prove
/// uniqueness from a complete provider-log snapshot.
pub(crate) fn has_unaliased_archive_bound_legacy_row_candidate(
    events: &[AgentChatEvent],
    path: &Path,
) -> bool {
    let log_path = path.to_string_lossy();
    events.iter().any(|event| {
        event.metadata["provider_log"] == true
            && event.metadata["log_path"].as_str() == Some(log_path.as_ref())
            && !event.provider.eq_ignore_ascii_case("claude")
            && event
                .metadata
                .get(PROVIDER_LOG_ROW_OFFSET_METADATA_KEY)
                .and_then(Value::as_u64)
                .is_some()
            && event.sequence.is_some()
            && requires_provider_log_row_identity(event)
            && !has_legacy_alias(event, &legacy_provider_log_event_id(event, path))
    })
}

/// Attach aliases from pre-offset archive rows only when both the archived
/// source sequence and the current batch identify exactly one observation.
/// Repeated equal provider rows therefore remain distinct.
pub(crate) fn attach_archive_bound_legacy_row_aliases(
    events: &mut [AgentChatEvent],
    path: &Path,
    archived_events: &[AgentChatEvent],
) {
    let log_path = path.to_string_lossy();
    let eligible = |event: &AgentChatEvent, require_offset: bool| {
        event.metadata["provider_log"] == true
            && event.metadata["log_path"].as_str() == Some(log_path.as_ref())
            && !event.provider.eq_ignore_ascii_case("claude")
            && event
                .metadata
                .get(PROVIDER_LOG_ROW_OFFSET_METADATA_KEY)
                .and_then(Value::as_u64)
                .is_some()
                == require_offset
            && event.sequence.is_some()
            && requires_provider_log_row_identity(event)
    };
    let row_key = |event: &AgentChatEvent| {
        Some((
            event.session_id.clone(),
            event.provider.clone(),
            legacy_provider_log_event_id(event, path),
            event.sequence?,
        ))
    };

    let mut current_counts = HashMap::<(String, String, String, u64), usize>::new();
    for event in events.iter().filter(|event| eligible(event, true)) {
        if let Some(key) = row_key(event) {
            *current_counts.entry(key).or_default() += 1;
        }
    }

    let mut archived_counts = HashMap::<(String, String, String, u64), usize>::new();
    for event in archived_events
        .iter()
        .filter(|event| eligible(event, false))
    {
        let Some(key) = row_key(event) else {
            continue;
        };
        if has_legacy_alias(event, &key.2) {
            *archived_counts.entry(key).or_default() += 1;
        }
    }

    let mut upgraded_archive_counts =
        HashMap::<((String, String, String, u64), String), usize>::new();
    for event in archived_events.iter().filter(|event| eligible(event, true)) {
        let Some(key) = row_key(event) else {
            continue;
        };
        if has_legacy_alias(event, &key.2) {
            *upgraded_archive_counts
                .entry((
                    key,
                    event.metadata["chat_source_ref"]
                        .as_str()
                        .unwrap_or(&event.id)
                        .to_string(),
                ))
                .or_default() += 1;
        }
    }

    for event in events.iter_mut().filter(|event| eligible(event, true)) {
        let Some(key) = row_key(event) else {
            continue;
        };
        let archived_pre_offset_count = archived_counts.get(&key).copied().unwrap_or_default();
        let archived_upgraded_count = upgraded_archive_counts
            .get(&(
                key.clone(),
                event.metadata["chat_source_ref"]
                    .as_str()
                    .unwrap_or(&event.id)
                    .to_string(),
            ))
            .copied()
            .unwrap_or_default();
        if current_counts.get(&key) != Some(&1)
            || archived_pre_offset_count + archived_upgraded_count != 1
            || key.2 == event.id
            || has_legacy_alias(event, &key.2)
        {
            continue;
        }
        if let Some(aliases) = event
            .metadata
            .as_object_mut()
            .map(|metadata| {
                metadata
                    .entry("legacy_event_ids")
                    .or_insert_with(|| serde_json::json!([]))
            })
            .and_then(Value::as_array_mut)
        {
            aliases.push(serde_json::json!(key.2));
        }
    }
}

fn has_legacy_alias(event: &AgentChatEvent, legacy_id: &str) -> bool {
    event.id == legacy_id
        || event.metadata["legacy_event_ids"]
            .as_array()
            .is_some_and(|aliases| {
                aliases
                    .iter()
                    .any(|alias| alias.as_str() == Some(legacy_id))
            })
}

pub(crate) fn stable_provider_log_event_id(event: &AgentChatEvent, path: &Path) -> String {
    with_provider_log_row_identity(event, stable_provider_log_event_id_inner(event, path))
}

pub(crate) fn with_provider_log_row_identity(event: &AgentChatEvent, stable_id: String) -> String {
    if let Some(provider_event_id) = provider_event_id(event) {
        if event.turn_id.as_deref() == Some(provider_event_id) {
            // The pre-offset identity already hashes this native per-event ID.
            return stable_id;
        }
        let mut hash = Sha256::new();
        hash.update(stable_id.as_bytes());
        hash.update(b"\0provider-event-id-v1\0");
        hash.update(provider_event_id.as_bytes());
        return format!(
            "{}:provider_log:{}",
            event.session_id,
            hash.finalize()
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }
    if !requires_provider_log_row_identity(event) {
        return stable_id;
    }
    let Some(offset) = provider_log_row_offset(event) else {
        return stable_id;
    };
    let mut hash = Sha256::new();
    hash.update(stable_id.as_bytes());
    hash.update(b"\0provider-log-row-offset-v1\0");
    hash.update(offset.to_be_bytes());
    format!(
        "{}:provider_log:{}",
        event.session_id,
        hash.finalize()
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// Reconstructs the field-derived ID used before source-row identity was
/// added. Callers may use it only where a native identity bridge proves the
/// archived alias belongs to this observation.
pub(crate) fn legacy_provider_log_event_id(event: &AgentChatEvent, path: &Path) -> String {
    stable_provider_log_event_id_inner(event, path)
}

pub(crate) fn requires_provider_log_row_identity(event: &AgentChatEvent) -> bool {
    if is_codex_user_message_mirror(event) {
        return false;
    }
    provider_event_id(event).is_none()
}

/// Only attach an old field-derived ID when that ID contains the native
/// per-event identifier. A provider UUID stored only in metadata cannot make
/// an otherwise shared legacy ID safe to alias.
pub(crate) fn can_alias_legacy_provider_log_event_id(event: &AgentChatEvent) -> bool {
    provider_event_id(event)
        .zip(event.turn_id.as_deref())
        .is_some_and(|(provider_event_id, turn_id)| provider_event_id == turn_id)
}

fn provider_event_id(event: &AgentChatEvent) -> Option<&str> {
    event
        .metadata
        .get(PROVIDER_EVENT_ID_METADATA_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|event_id| !event_id.is_empty())
}

fn provider_log_row_offset(event: &AgentChatEvent) -> Option<u64> {
    event
        .metadata
        .get(PROVIDER_LOG_ROW_OFFSET_METADATA_KEY)
        .and_then(Value::as_u64)
        .or(event.sequence)
}

/// Return absolute byte offsets for each physical source row.
pub(crate) fn provider_log_row_offsets(content: &[u8], absolute_start_offset: u64) -> Vec<u64> {
    let mut offsets = Vec::new();
    let mut offset = absolute_start_offset;
    for line in content.split_inclusive(|byte| *byte == b'\n') {
        offsets.push(offset);
        offset = offset.saturating_add(line.len() as u64);
    }
    offsets
}

/// Bind normalized events to their absolute byte position in the append-only
/// provider source. Sequence numbers map normalizer rows across bounded batches.
pub(crate) fn attach_provider_log_row_offsets(
    events: &mut [AgentChatEvent],
    row_offsets: &[u64],
    first_sequence: u64,
) {
    for event in events {
        if event.metadata[PROVIDER_LOG_ROW_OFFSET_METADATA_KEY].is_number() {
            continue;
        }
        if let Some(offset) = event
            .sequence
            .and_then(|sequence| sequence.checked_sub(first_sequence))
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| row_offsets.get(index))
        {
            event.metadata[PROVIDER_LOG_ROW_OFFSET_METADATA_KEY] = serde_json::json!(offset);
        }
    }
}

fn stable_provider_log_event_id_inner(event: &AgentChatEvent, path: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(event.session_id.as_bytes());
    hash.update(b"\0");
    hash.update(event.provider.as_bytes());
    hash.update(b"\0");
    hash.update(path.to_string_lossy().as_bytes());
    hash.update(b"\0");
    hash.update(format!("{:?}", event.kind).as_bytes());
    hash.update(b"\0");
    hash.update(format!("{:?}", event.role).as_bytes());
    hash.update(b"\0");
    if is_codex_user_message_mirror(event) {
        if let Some(provider_turn_id) = codex_user_message_mirror_provider_turn_id(event) {
            hash.update(b"codex-user-message-mirror-provider-turn\0");
            hash.update(provider_turn_id.as_bytes());
        } else if let Some(sequence) = event.sequence {
            // The capture normalizer persists its per-source line sequence
            // alongside the append-only cursor, so an unbound mirror remains
            // stable across retries without being joined by its text.
            hash.update(b"codex-user-message-mirror-log-sequence\0");
            hash.update(sequence.to_string().as_bytes());
        } else {
            // Normalized provider events always have a sequence. Keep a
            // non-text identity fallback for callers that construct events
            // directly; never regress to the shared legacy text hash.
            hash.update(b"codex-user-message-mirror-event-id\0");
            hash.update(event.id.as_bytes());
        }
        hash.update(b"\0");
    }
    for value in [
        event.turn_id.as_deref(),
        event.created_at.as_deref(),
        event.source.as_deref(),
        event.title.as_deref(),
        event.command.as_deref(),
        event.text.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        hash.update(value.as_bytes());
        hash.update(b"\0");
    }
    format!(
        "{}:provider_log:{}",
        event.session_id,
        hash.finalize()
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn codex_user_message_mirror_provider_turn_id(event: &AgentChatEvent) -> Option<&str> {
    is_codex_user_message_mirror(event).then(|| {
        event
            .metadata
            .get("provider_turn_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })?
}

fn is_codex_user_message_mirror(event: &AgentChatEvent) -> bool {
    use wardian_core::models::chat::AgentChatRole;

    event.provider == "codex"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && event.source.as_deref() == Some("event_msg")
        && event.metadata["raw_type"] == "user_message"
        && event.metadata["input_origin"] == "human_input"
}
