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
    use wardian_core::models::chat::AgentChatRole;
    if a.provider != "codex"
        || b.provider != "codex"
        || a.session_id != b.session_id
        || a.role != Some(AgentChatRole::Assistant)
        || b.role != Some(AgentChatRole::Assistant)
    {
        return false;
    }
    let (stream, completed) = match (a.source.as_deref(), b.source.as_deref()) {
        (Some("event_msg"), Some("response_item" | "item.completed")) => (a, b),
        (Some("response_item" | "item.completed"), Some("event_msg")) => (b, a),
        _ => return false,
    };
    if stream.turn_id.is_some() || completed.turn_id.as_deref().is_none_or(str::is_empty) {
        return false;
    }
    for key in [
        "request_root_id",
        "source_path",
        "log_path",
        "conversation_archive_id",
    ] {
        if let (Some(left), Some(right)) = (a.metadata.get(key), b.metadata.get(key)) {
            if left != right {
                return false;
            }
        }
    }
    true
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
