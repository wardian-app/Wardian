//! Project native OpenCode SQLite tool parts without inventing human turns.
use super::{AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus, OpencodeDbPart};
use serde_json::{json, Value};
use std::path::Path;

/// The SQL reader already restricts both message and part to the selected session.
/// Pending parts have no authoritative input yet; results exist only at termination.
pub(super) fn project(
    agent: &str,
    session: &str,
    row: &OpencodeDbPart,
    request_root: Option<&str>,
    db_path: &Path,
    sequence: u64,
) -> Vec<AgentChatEvent> {
    let (Ok(part), Ok(message)) = (
        serde_json::from_str::<Value>(&row.part_data),
        serde_json::from_str::<Value>(&row.message_data),
    ) else {
        return Vec::new();
    };
    if part["type"] != "tool" || message["role"] != "assistant" {
        return Vec::new();
    }
    // Some versions repeat DB identity in the JSON. Conflicting identity fails closed.
    for (value, expected) in [
        (&part["sessionID"], session),
        (&message["sessionID"], session),
        (&part["messageID"], row.message_id.as_str()),
        (&part["id"], row.part_id.as_str()),
        (&message["id"], row.message_id.as_str()),
    ] {
        if !value.is_null() && value.as_str() != Some(expected) {
            return Vec::new();
        }
    }
    let (Some(tool), Some(call_id), Some(input), Some(state)) = (
        part["tool"].as_str().filter(|v| !v.is_empty()),
        part["callID"].as_str().filter(|v| !v.is_empty()),
        part["state"]["input"].as_object(),
        part["state"]["status"].as_str(),
    ) else {
        return Vec::new();
    };
    if !matches!(state, "running" | "completed" | "error") {
        return Vec::new();
    }
    let mut metadata = json!({
        "provider_log": true, "raw_type": "tool", "opencode_session_id": session,
        "part_id": row.part_id, "tool_name": tool, "tool_call_id": call_id,
        "tool_input": input, "source_path": db_path.to_string_lossy(),
        "part_time_created": row.part_time_created,
        "message_time_created": row.message_time_created,
        "causal_ref": format!("provider:message:{}", row.message_id),
    });
    if let Some(root) = message["parentID"]
        .as_str()
        .filter(|v| !v.is_empty())
        .or(request_root)
    {
        metadata["request_root_id"] = json!(root);
    }
    let event = AgentChatEvent {
        id: format!("{agent}:opencode_db:{session}:{}:call", row.part_id),
        session_id: agent.into(),
        provider: "opencode".into(),
        kind: AgentChatEventKind::ToolCall,
        role: Some(AgentChatRole::Assistant),
        text: None,
        title: Some(tool.into()),
        status: None,
        turn_id: Some(row.message_id.clone()),
        source: Some("opencode_db".into()),
        command: input
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned),
        path: input
            .get("filePath")
            .and_then(Value::as_str)
            .map(str::to_owned),
        exit_code: None,
        language: None,
        created_at: timestamp(&part["state"]["time"]["start"]),
        sequence: Some(sequence),
        metadata,
    };
    let mut events = vec![event.clone()];
    let terminal = match state {
        "completed" => part["state"]["output"]
            .as_str()
            .map(|s| (s, AgentChatStatus::Succeeded)),
        "error" => part["state"]["error"]
            .as_str()
            .map(|s| (s, AgentChatStatus::Failed)),
        _ => None,
    };
    if let Some((text, status)) = terminal {
        let mut result = event;
        result.id = format!("{agent}:opencode_db:{session}:{}:result", row.part_id);
        result.kind = AgentChatEventKind::ToolResult;
        result.role = Some(AgentChatRole::Tool);
        result.text = Some(text.into());
        result.status = Some(status);
        result.sequence = Some(sequence + 1);
        result.created_at = timestamp(&part["state"]["time"]["end"]);
        result.metadata["causal_ref"] = json!(format!("provider:tool:{call_id}"));
        events.push(result);
    }
    events
}

fn timestamp(value: &Value) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(value.as_i64()?).map(|time| time.to_rfc3339())
}

#[cfg(test)]
#[path = "chat_opencode_tools_tests.rs"]
mod tests;
