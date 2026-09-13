//! Pi durable transcript identity. Session entry IDs belong to the envelope,
//! independently of message IDs and tool-call IDs in the nested payload.

/// Returns only a durable message entry ID, never a session or parent ID.
pub(crate) fn message_entry_id(record: &serde_json::Value) -> Option<String> {
    if record.get("type")?.as_str()? != "message" {
        return None;
    }
    record
        .get("id")?
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::chat_transcript::{normalize_chat_line, normalize_chat_lines};
    use serde_json::json;
    use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};

    const REAL_SESSION: &str = include_str!("fixtures/real-headless-session.jsonl");

    #[test]
    fn retained_real_pi_request_uses_its_durable_entry_as_root() {
        let events = normalize_chat_lines("wardian-agent", "pi", REAL_SESSION.lines());
        let request = events
            .iter()
            .find(|event| event.role == Some(AgentChatRole::User))
            .unwrap();
        assert_eq!(request.text.as_deref(), Some("Return exactly WARDIAN_HEADLESS_AUTOMATION_PI_72244-1788774000195 and no other text."));
        assert_eq!(request.metadata["input_origin"], "human_input");
        assert_eq!(request.metadata["input_purpose"], "request");
        assert_eq!(request.metadata["request_root_id"], "ec1b3195");
        assert_eq!(request.turn_id.as_deref(), Some("ec1b3195"));
        let answer = events
            .iter()
            .find(|event| event.role == Some(AgentChatRole::Assistant))
            .unwrap();
        assert_eq!(answer.turn_id.as_deref(), Some("1f0e1517"));
        assert_eq!(
            answer.text.as_deref(),
            Some("WARDIAN_HEADLESS_AUTOMATION_PI_72244-1788774000195")
        );
        assert!(answer.metadata.get("input_origin").is_none());

        // A bounded log tail may start at this user record. Its root must not
        // depend on the session header, preceding model records, or line index.
        let user_line = REAL_SESSION
            .lines()
            .find(|line| line.contains("ec1b3195") && line.contains("\"role\":\"user\""))
            .unwrap();
        let tail_request = normalize_chat_line("wardian-agent", "pi", user_line, 91).unwrap();
        assert_eq!(
            tail_request.metadata["request_root_id"],
            request.metadata["request_root_id"]
        );
    }

    #[test]
    fn pi_entry_identity_does_not_invent_roots_for_unidentified_stream_messages() {
        for record in [
            json!({"type":"session", "id":"session-id"}),
            json!({"type":"message_end", "id":"event-id"}),
            json!({"type":"message", "parentId":"parent-id"}),
            json!({"type":"message", "id":" "}),
            json!({"type":"message", "id":7}),
        ] {
            assert_eq!(message_entry_id(&record), None);
        }
        let stream =
            r#"{"type":"message_end","message":{"role":"user","id":"nested-id","content":"task"}}"#;
        let event = normalize_chat_line("agent", "pi", stream, 1).unwrap();
        assert_eq!(event.metadata["request_root_id"], "nested-id");
        let unidentified = r#"{"type":"message_end","message":{"role":"user","content":"task"}}"#;
        let event = normalize_chat_line("agent", "pi", unidentified, 1).unwrap();
        assert!(event.metadata.get("request_root_id").is_none());
        let durable = r#"{"type":"message","id":"entry-id","message":{"role":"user","id":"nested-id","content":"task"}}"#;
        let event = normalize_chat_line("agent", "pi", durable, 1).unwrap();
        assert_eq!(event.metadata["request_root_id"], "entry-id");
    }

    #[test]
    fn pi_envelope_identity_preserves_tool_call_result_pairing() {
        // Supplementary synthetic tool records: the retained real harness turn
        // performed no tools, so this is not claimed as real tool acceptance.
        let lines = [
            r#"{"type":"message","id":"request-entry","message":{"role":"user","content":"Read scratch.txt"}}"#,
            r#"{"type":"message","id":"call-entry","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"scratch.txt"}}]}}"#,
            r#"{"type":"message","id":"result-entry","message":{"role":"toolResult","toolCallId":"call-1","toolName":"read","content":[{"type":"text","text":"scratch data"}],"isError":false}}"#,
        ];
        let events = normalize_chat_lines("agent", "pi", lines);
        assert_eq!(events[0].metadata["request_root_id"], "request-entry");
        assert_eq!(events[1].kind, AgentChatEventKind::ToolCall);
        assert_eq!(events[1].turn_id.as_deref(), Some("call-1"));
        assert_eq!(events[1].metadata["files_read"][0], "scratch.txt");
        assert_eq!(events[2].kind, AgentChatEventKind::ToolResult);
        assert_eq!(events[2].role, Some(AgentChatRole::Tool));
        assert_eq!(events[2].turn_id, events[1].turn_id);
        assert!(events[2].metadata.get("input_origin").is_none());
        assert_eq!(
            events
                .iter()
                .filter(|event| event.metadata["input_origin"] == "human_input")
                .count(),
            1
        );
    }
}
