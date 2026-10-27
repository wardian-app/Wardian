//! Compatibility identities proven by a complete native log observation.
use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

use sha2::{Digest, Sha256};

/// Codex emits an identity-less `event_msg` mirror and an identified completed
/// assistant message. Preserve that established cross-source normalization while
/// keeping two identified native messages (including equal answers) distinct.
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
        serde_json::json!(stable_provider_log_event_id(&original, path));
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
        let old_id = stable_provider_log_event_id(&legacy, path);
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

pub(crate) fn stable_provider_log_event_id(event: &AgentChatEvent, path: &Path) -> String {
    stable_provider_log_event_id_inner(event, path)
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
