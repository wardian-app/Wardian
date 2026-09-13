//! Bounded SQLite tool projection verified against paired Antigravity 1.1.27
//! SQLite/JSONL records. Unknown step layouts are deliberately not projected.

use super::{
    protobuf_message_at_path, protobuf_string_at_path, protobuf_varint, protobuf_varint_field,
};
use crate::providers::chat_transcript::normalize_antigravity;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

/// Reads only observed planner-call and generic-result layouts from the already
/// selected database. The database remains the sole source and log binding.
pub(crate) fn load_tools(
    session_id: &str,
    provider: &str,
    path: &Path,
) -> Result<Vec<AgentChatEvent>, rusqlite::Error> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut statement = connection.prepare(
        "SELECT idx, step_type, step_payload FROM steps WHERE step_type IN (15, 132) ORDER BY idx",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (index, step_type, payload) = row?;
        let Ok(index) = u64::try_from(index) else {
            continue;
        };
        let source = protobuf_message_at_path(&payload, &[5])
            .and_then(|metadata| protobuf_varint_field(metadata, 3));
        // Tool-shaped bytes from other sources are not evidence of model tools.
        if source != Some(2) {
            continue;
        }
        for (ordinal, record, call_id) in tool_records(step_type, &payload) {
            let mut record = record;
            record["step_index"] = json!(index);
            let Some(mut event) = normalize_antigravity(session_id, provider, &record, 0) else {
                continue;
            };
            event.role = Some(if event.kind == AgentChatEventKind::ToolResult {
                AgentChatRole::Tool
            } else {
                AgentChatRole::Assistant
            });
            // DONE is completion, not proof of a successful tool outcome.
            event.status = Some(AgentChatStatus::Unknown);
            event.source = Some("conversation_database".to_string());
            event.metadata["provider_log"] = json!(true);
            event.metadata["log_source"] = json!("antigravity_conversation_database");
            event.metadata["log_path"] = json!(path.to_string_lossy());
            event.metadata["step_index"] = json!(index);
            event.metadata["provider_step_type"] = json!(step_type);
            event.metadata["provider_step_source"] = json!(source);
            event.metadata["provider_step_status"] = json!(protobuf_varint_field(&payload, 4));
            event.metadata["tool_ordinal"] = json!(ordinal);
            if let Some(call_id) = call_id {
                event.metadata["tool_call_id"] = json!(call_id);
            }
            // Output and arguments can evolve while a step is running. Identity
            // uses the provider's step location, never mutable content or the
            // sequence assigned after interleaving with existing messages.
            let mut hash = Sha256::new();
            for part in [
                session_id,
                provider,
                &path.to_string_lossy(),
                &index.to_string(),
                &format!("{:?}", event.kind),
                &ordinal.to_string(),
            ] {
                hash.update(part.as_bytes());
                hash.update(b"\0");
            }
            event.id = format!("{session_id}:antigravity_tool:{:x}", hash.finalize());
            events.push(event);
        }
    }
    Ok(events)
}

fn tool_records(step_type: i64, payload: &[u8]) -> Vec<(usize, Value, Option<String>)> {
    match step_type {
        15 => protobuf_message_at_path(payload, &[20])
            .and_then(|planner| repeated_messages(planner, 7))
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .filter_map(|(ordinal, call)| {
                let name = protobuf_string_at_path(call, &[2])?;
                if name.trim().is_empty() {
                    return None;
                }
                let args: Value =
                    serde_json::from_str(&protobuf_string_at_path(call, &[3])?).ok()?;
                if !args.is_object() {
                    return None;
                }
                Some((
                    ordinal,
                    json!({"type": "PLANNER_RESPONSE", "source": "MODEL",
                    "tool_calls": [{"name": name, "args": args}]}),
                    protobuf_string_at_path(call, &[1]),
                ))
            })
            .collect(),
        132 => protobuf_string_at_path(payload, &[140, 2, 1])
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                vec![(
                    0,
                    json!({"type": "GENERIC", "source": "MODEL", "content": text}),
                    None,
                )]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Preserve repeated-field wire order and fail closed on truncated envelopes.
fn repeated_messages(bytes: &[u8], wanted: u32) -> Option<Vec<&[u8]>> {
    let mut offset = 0;
    let mut values = Vec::new();
    while offset < bytes.len() {
        let key = protobuf_varint(bytes, &mut offset)?;
        if key >> 3 == 0 {
            return None;
        }
        match key & 7 {
            0 => {
                protobuf_varint(bytes, &mut offset)?;
            }
            1 => offset = offset.checked_add(8)?,
            2 => {
                let length = usize::try_from(protobuf_varint(bytes, &mut offset)?).ok()?;
                let end = offset.checked_add(length)?;
                let value = bytes.get(offset..end)?;
                offset = end;
                if key >> 3 == u64::from(wanted) {
                    values.push(value);
                }
            }
            5 => offset = offset.checked_add(4)?,
            _ => return None,
        }
        if offset > bytes.len() {
            return None;
        }
    }
    Some(values)
}
