use wardian_core::control::WatchTranscriptMessage;

pub fn extract_transcript_message(
    provider_id: &str,
    raw_line: &str,
) -> Option<WatchTranscriptMessage> {
    let provider = provider_id.trim().to_ascii_lowercase();
    match provider.as_str() {
        "codex" => extract_codex(raw_line),
        "claude" => extract_claude(raw_line),
        "gemini" => extract_gemini(raw_line),
        "antigravity" => extract_antigravity(raw_line),
        "mock" => extract_mock(raw_line),
        "opencode" => extract_opencode(raw_line),
        _ => None,
    }
}

fn extract_codex(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    let msg_type = parsed.get("type")?.as_str()?;
    let (payload, source) = match msg_type {
        "response_item" => (parsed.get("payload")?, "response_item"),
        "event_msg" => (parsed.get("payload")?, "event_msg"),
        "item.completed" => (parsed.get("item")?, "item.completed"),
        _ => return None,
    };
    let payload_type = payload.get("type").and_then(|value| value.as_str())?;
    let role = payload.get("role").and_then(|value| value.as_str());
    let is_assistant = role == Some("assistant")
        || matches!(
            payload_type,
            "agent_message" | "assistant_message" | "message"
        );
    if !is_assistant {
        return None;
    }
    let text = extract_text(payload)?;
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "codex".to_string(),
        turn_id: parsed
            .get("turn_id")
            .or_else(|| payload.get("turn_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        source: Some(source.to_string()),
    })
}

fn extract_claude(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    if parsed.get("type").and_then(|value| value.as_str())? != "assistant" {
        return None;
    }
    let message = parsed.get("message").unwrap_or(&parsed);
    let text = extract_text(message)?;
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "claude".to_string(),
        turn_id: message
            .get("id")
            .or_else(|| parsed.get("message_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        source: Some("stream_json".to_string()),
    })
}

fn extract_mock(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    let msg_type = parsed.get("type")?.as_str()?;
    if !matches!(msg_type, "model" | "message" | "info") {
        return None;
    }
    if msg_type == "message" {
        let role = parsed.get("role").and_then(|value| value.as_str());
        if !matches!(role, Some("assistant" | "model")) {
            return None;
        }
    }
    let text = extract_text(&parsed)?;
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "mock".to_string(),
        turn_id: parsed
            .get("turn_id")
            .or_else(|| parsed.get("id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        source: Some(msg_type.to_string()),
    })
}

fn extract_gemini(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    let msg_type = gemini_message_kind(&parsed)?;
    match msg_type {
        "gemini" | "assistant" | "model" if gemini_completed_message(&parsed) => {}
        "message" => {
            let role = parsed.get("role").and_then(|value| value.as_str());
            if !matches!(role, Some("assistant" | "model")) {
                return None;
            }
            if !gemini_completed_message(&parsed) {
                return None;
            }
        }
        _ => return None,
    }

    let text = extract_text(&parsed)?;
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "gemini".to_string(),
        turn_id: parsed
            .get("id")
            .or_else(|| parsed.get("message_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        source: Some("gemini_log".to_string()),
    })
}

fn extract_opencode(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    if parsed.get("type").and_then(|value| value.as_str())? != "text" {
        return None;
    }
    let text = parsed
        .get("part")
        .and_then(extract_text)
        .or_else(|| extract_text(&parsed))?;
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "opencode".to_string(),
        turn_id: parsed
            .get("sessionID")
            .or_else(|| parsed.get("session_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        source: Some("stream_json".to_string()),
    })
}

fn extract_antigravity(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    if parsed.get("source").and_then(|value| value.as_str()) != Some("MODEL")
        || parsed.get("type").and_then(|value| value.as_str()) != Some("PLANNER_RESPONSE")
        || parsed.get("status").and_then(|value| value.as_str()) != Some("DONE")
    {
        return None;
    }
    let text = extract_text(&parsed)?;
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "antigravity".to_string(),
        turn_id: parsed
            .get("step_index")
            .and_then(|value| value.as_u64())
            .map(|value| value.to_string()),
        source: Some("transcript".to_string()),
    })
}

fn gemini_message_kind(value: &serde_json::Value) -> Option<&str> {
    value
        .get("type")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("role").and_then(|value| value.as_str()))
}

fn gemini_completed_message(value: &serde_json::Value) -> bool {
    value.get("tokens").is_some()
        || value.get("usage").is_some()
        || value.get("finishReason").is_some()
        || value.get("finish_reason").is_some()
}

fn extract_text(value: &serde_json::Value) -> Option<String> {
    let candidates = ["text", "content", "message", "summary"];
    for key in candidates {
        if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    if let Some(content) = value.get("content").and_then(|value| value.as_array()) {
        let parts = content
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .or_else(|| item.get("content"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
            })
            .collect::<Vec<_>>();
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_response_item_message_extracts_assistant_text() {
        let line = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Codex answer"}]},"turn_id":"turn-1"}"#;

        let message = extract_transcript_message("codex", line).unwrap();

        assert_eq!(message.role, "assistant");
        assert_eq!(message.text, "Codex answer");
        assert_eq!(message.provider, "codex");
        assert_eq!(message.turn_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn claude_assistant_content_text_block_extracts_assistant_text() {
        let line = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"Claude answer"}]}}"#;

        let message = extract_transcript_message("claude", line).unwrap();

        assert_eq!(message.role, "assistant");
        assert_eq!(message.text, "Claude answer");
        assert_eq!(message.provider, "claude");
        assert_eq!(message.turn_id.as_deref(), Some("msg-1"));
    }

    #[test]
    fn mock_model_event_extracts_assistant_text() {
        let line = r#"{"type":"model","content":"Mock answer","turn_id":"turn-7"}"#;

        let message = extract_transcript_message("mock", line).unwrap();

        assert_eq!(message.role, "assistant");
        assert_eq!(message.text, "Mock answer");
        assert_eq!(message.provider, "mock");
        assert_eq!(message.turn_id.as_deref(), Some("turn-7"));
    }

    #[test]
    fn opencode_text_part_extracts_assistant_text() {
        let line = r#"{"type":"text","sessionID":"ses_test","part":{"type":"text","text":"OpenCode answer"}}"#;

        let message = extract_transcript_message("opencode", line).unwrap();

        assert_eq!(message.role, "assistant");
        assert_eq!(message.text, "OpenCode answer");
        assert_eq!(message.provider, "opencode");
        assert_eq!(message.turn_id.as_deref(), Some("ses_test"));
    }

    #[test]
    fn antigravity_planner_response_extracts_assistant_text() {
        let line = r#"{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-05-20T09:21:54Z","content":"Antigravity answer"}"#;

        let message = extract_transcript_message("antigravity", line).unwrap();

        assert_eq!(message.role, "assistant");
        assert_eq!(message.text, "Antigravity answer");
        assert_eq!(message.provider, "antigravity");
        assert_eq!(message.turn_id.as_deref(), Some("2"));
        assert_eq!(message.source.as_deref(), Some("transcript"));
    }

    #[test]
    fn antigravity_user_input_does_not_extract_transcript() {
        let line = r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"hello"}"#;

        assert!(extract_transcript_message("antigravity", line).is_none());
    }

    #[test]
    fn gemini_completed_model_record_extracts_assistant_text() {
        let line = r#"{"id":"gem-msg-1","type":"model","content":"Gemini answer","tokens":{"input":10,"output":2,"total":12}}"#;

        let message = extract_transcript_message("gemini", line).unwrap();

        assert_eq!(message.role, "assistant");
        assert_eq!(message.text, "Gemini answer");
        assert_eq!(message.provider, "gemini");
        assert_eq!(message.turn_id.as_deref(), Some("gem-msg-1"));
    }

    #[test]
    fn gemini_partial_model_chunk_does_not_extract_transcript() {
        for line in [
            r#"{"id":"gem-msg-1","type":"model","content":"partial chunk"}"#,
            r#"{"id":"gem-msg-2","type":"gemini","content":"partial chunk"}"#,
            r#"{"id":"gem-msg-3","type":"assistant","content":"partial chunk"}"#,
        ] {
            assert!(extract_transcript_message("gemini", line).is_none());
        }
    }

    #[test]
    fn user_prompt_echo_and_tool_events_do_not_extract_transcript() {
        assert!(
            extract_transcript_message("mock", r#"{"type":"user","content":"hello"}"#).is_none()
        );
        assert!(extract_transcript_message(
            "codex",
            r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{}"}}"#
        )
        .is_none());
        assert!(extract_transcript_message(
            "claude",
            r#"{"type":"user","message":{"content":[{"type":"text","text":"hello"}]}}"#
        )
        .is_none());
        assert!(
            extract_transcript_message("gemini", r#"{"type":"user","content":"hello"}"#).is_none()
        );
        assert!(extract_transcript_message(
            "antigravity",
            r#"{"source":"USER_EXPLICIT","type":"USER_INPUT","content":"hello"}"#
        )
        .is_none());
    }
}
