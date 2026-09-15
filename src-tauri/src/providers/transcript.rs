use wardian_core::control::{WatchTranscriptMessage, WatchTranscriptProvenance};
use wardian_core::models::AgentEvent;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CodexWatchBindingState {
    provider_session_id: Option<String>,
    source_path: Option<String>,
    active_turn_id: Option<String>,
    completed_turn_id: Option<String>,
}

impl CodexWatchBindingState {
    pub(crate) fn set_source(&mut self, provider_session_id: &str, source_path: &str) {
        let provider_session_id = provider_session_id.trim();
        let source_path = source_path.trim();
        if provider_session_id.is_empty() || source_path.is_empty() {
            self.reset();
            return;
        }

        if self.provider_session_id.as_deref() != Some(provider_session_id)
            || self.source_path.as_deref() != Some(source_path)
        {
            self.active_turn_id = None;
            self.completed_turn_id = None;
        }
        self.provider_session_id = Some(provider_session_id.to_string());
        self.source_path = Some(source_path.to_string());
    }

    pub(crate) fn reset(&mut self) {
        self.provider_session_id = None;
        self.source_path = None;
        self.active_turn_id = None;
        self.completed_turn_id = None;
    }

    fn reset_turn(&mut self) {
        self.active_turn_id = None;
        self.completed_turn_id = None;
    }

    pub(crate) fn observe_record(&mut self, raw_line: &str, event: Option<&AgentEvent>) {
        let parsed: serde_json::Value = match serde_json::from_str(raw_line) {
            Ok(parsed) => parsed,
            Err(_) => {
                self.reset_turn();
                return;
            }
        };
        let record_type = parsed.get("type").and_then(|value| value.as_str());
        let payload_type = parsed
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(|value| value.as_str());
        let explicit_turn_id = codex_record_turn_id(&parsed).map(str::trim);

        if record_type == Some("event_msg") && payload_type == Some("task_started") {
            match (event, explicit_turn_id) {
                (Some(AgentEvent::TurnStarted { turn_id }), Some(raw_turn_id))
                    if !raw_turn_id.is_empty()
                        && !turn_id.trim().is_empty()
                        && raw_turn_id == turn_id.trim()
                        && self.has_source() =>
                {
                    self.active_turn_id = Some(raw_turn_id.to_string());
                    self.completed_turn_id = None;
                }
                _ => self.reset_turn(),
            }
            return;
        }

        match event {
            Some(AgentEvent::TurnStarted { turn_id }) => {
                let Some(raw_turn_id) = explicit_turn_id.filter(|turn_id| !turn_id.is_empty())
                else {
                    self.reset_turn();
                    return;
                };
                if !self.has_source() || raw_turn_id != turn_id.trim() || turn_id.trim().is_empty()
                {
                    self.reset_turn();
                    return;
                }
                self.active_turn_id = Some(raw_turn_id.to_string());
                self.completed_turn_id = None;
                return;
            }
            Some(AgentEvent::TurnCompleted) => {
                let Some(raw_turn_id) = explicit_turn_id.filter(|turn_id| !turn_id.is_empty())
                else {
                    self.reset_turn();
                    return;
                };
                if !self.has_source() {
                    self.reset_turn();
                    return;
                }
                if self.active_turn_id.as_deref() == Some(raw_turn_id) {
                    self.completed_turn_id = Some(raw_turn_id.to_string());
                    self.active_turn_id = None;
                } else if self.completed_turn_id.as_deref() != Some(raw_turn_id) {
                    self.reset_turn();
                }
                return;
            }
            Some(AgentEvent::TurnInterrupted) => {
                self.reset_turn();
                return;
            }
            Some(AgentEvent::UserQuery) if self.active_turn_id.is_none() => {
                self.reset_turn();
                return;
            }
            _ => {}
        }

        if let Some(explicit_turn_id) = explicit_turn_id.filter(|turn_id| !turn_id.is_empty()) {
            if self
                .active_turn_id
                .as_deref()
                .or(self.completed_turn_id.as_deref())
                .is_some_and(|bound_turn_id| bound_turn_id != explicit_turn_id)
            {
                self.reset_turn();
            }
        }
    }

    pub(crate) fn extract_message(&mut self, raw_line: &str) -> Option<WatchTranscriptMessage> {
        let mut message = extract_codex(raw_line)?;
        let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
        let explicit_provider_turn_id = codex_record_turn_id(&parsed)
            .map(str::trim)
            .filter(|turn_id| !turn_id.is_empty());
        let is_identityless_agent_message = message.turn_id.is_none()
            && parsed.get("type").and_then(|value| value.as_str()) == Some("event_msg")
            && parsed
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(|value| value.as_str())
                == Some("agent_message")
            && explicit_provider_turn_id.is_none();
        let is_response_item_message = message.source.as_deref() == Some("response_item")
            && message.turn_id.is_some()
            && explicit_provider_turn_id.is_some();

        if is_identityless_agent_message || is_response_item_message {
            let turn_id = self.active_turn_id.clone().or_else(|| {
                if is_identityless_agent_message {
                    self.completed_turn_id.take()
                } else {
                    self.completed_turn_id.clone()
                }
            });
            if let (Some(provider_session_id), Some(source_path), Some(provider_turn_id)) = (
                self.provider_session_id.as_ref(),
                self.source_path.as_ref(),
                turn_id,
            ) {
                if is_identityless_agent_message
                    || explicit_provider_turn_id == Some(provider_turn_id.as_str())
                {
                    message.provider_provenance = Some(WatchTranscriptProvenance {
                        provider_session_id: provider_session_id.clone(),
                        source_path: source_path.clone(),
                        provider_turn_id,
                    });
                }
            }
        }

        Some(message)
    }

    fn has_source(&self) -> bool {
        self.provider_session_id.is_some() && self.source_path.is_some()
    }
}

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
        "pi" => extract_pi(raw_line),
        _ => None,
    }
}

fn extract_pi(raw_line: &str) -> Option<WatchTranscriptMessage> {
    let parsed: serde_json::Value = serde_json::from_str(raw_line).ok()?;
    let kind = parsed.get("type").and_then(|value| value.as_str())?;
    let message = match kind {
        "message" | "message_end" => parsed.get("message")?,
        _ => return None,
    };
    if message.get("role").and_then(|value| value.as_str()) != Some("assistant") {
        return None;
    }
    if kind == "message" {
        let stop_reason = message.get("stopReason").and_then(|value| value.as_str());
        if !matches!(stop_reason, Some("stop" | "length")) {
            return None;
        }
    }
    let text = extract_text(message)?;
    Some(WatchTranscriptMessage {
        role: "assistant".into(),
        text,
        provider: "pi".into(),
        turn_id: message
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        source: Some(
            if kind == "message" {
                "session_jsonl"
            } else {
                "json_mode"
            }
            .into(),
        ),
        provider_provenance: None,
    })
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
    let is_assistant = match role {
        Some("assistant" | "model") => true,
        Some(_) => false,
        None => matches!(payload_type, "agent_message" | "assistant_message"),
    };
    if !is_assistant {
        return None;
    }
    let text = extract_text(payload)?;
    let turn_id = if source == "response_item" {
        payload
            .get("id")
            .or_else(|| payload.get("message_id"))
            .or_else(|| parsed.get("message_id"))
            .or_else(|| parsed.get("turn_id"))
            .or_else(|| payload.get("turn_id"))
    } else {
        parsed.get("turn_id").or_else(|| payload.get("turn_id"))
    }
    .and_then(|value| value.as_str())
    .map(str::to_string);
    Some(WatchTranscriptMessage {
        role: "assistant".to_string(),
        text,
        provider: "codex".to_string(),
        turn_id,
        source: Some(source.to_string()),
        provider_provenance: None,
    })
}

fn codex_record_turn_id(parsed: &serde_json::Value) -> Option<&str> {
    [Some(parsed), parsed.get("payload"), parsed.get("item")]
        .into_iter()
        .flatten()
        .find_map(codex_value_turn_id)
}

fn codex_value_turn_id(value: &serde_json::Value) -> Option<&str> {
    value
        .get("turn_id")
        .and_then(|turn_id| turn_id.as_str())
        .filter(|turn_id| !turn_id.trim().is_empty())
        .or_else(|| {
            value
                .get("internal_chat_message_metadata_passthrough")
                .and_then(|metadata| metadata.get("turn_id"))
                .and_then(|turn_id| turn_id.as_str())
                .filter(|turn_id| !turn_id.trim().is_empty())
        })
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|metadata| metadata.get("turn_id"))
                .and_then(|turn_id| turn_id.as_str())
                .filter(|turn_id| !turn_id.trim().is_empty())
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
        provider_provenance: None,
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
        provider_provenance: None,
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
        provider_provenance: None,
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
        provider_provenance: None,
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
        provider_provenance: None,
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

    fn codex_task_started(turn_id: &str) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"{turn_id}"}}}}"#
        )
    }

    fn codex_task_complete(turn_id: &str) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"{turn_id}"}}}}"#
        )
    }

    fn codex_agent_message(text: &str) -> String {
        format!(r#"{{"type":"event_msg","payload":{{"type":"agent_message","message":"{text}"}}}}"#)
    }

    fn codex_watch_state() -> CodexWatchBindingState {
        let mut state = CodexWatchBindingState::default();
        state.set_source("codex-session", "codex-session.jsonl");
        state
    }

    #[test]
    fn codex_watch_binds_identityless_agent_message_to_task_started_observation() {
        let mut state = codex_watch_state();
        let task_started = codex_task_started("turn-a");
        state.observe_record(
            &task_started,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-a".to_string(),
            }),
        );

        let message = state
            .extract_message(&codex_agent_message("Codex answer"))
            .expect("agent message");

        assert_eq!(message.turn_id, None);
        assert_eq!(message.source.as_deref(), Some("event_msg"));
        assert_eq!(
            message.provider_provenance,
            Some(WatchTranscriptProvenance {
                provider_session_id: "codex-session".to_string(),
                source_path: "codex-session.jsonl".to_string(),
                provider_turn_id: "turn-a".to_string(),
            })
        );
    }

    #[test]
    fn codex_watch_response_item_keeps_message_id_separate_from_provider_turn() {
        let mut state = codex_watch_state();
        let task_started = codex_task_started("turn-a");
        state.observe_record(
            &task_started,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-a".to_string(),
            }),
        );

        let message = state
            .extract_message(
                r#"{"type":"response_item","payload":{"type":"message","id":"msg-native","role":"assistant","content":[{"type":"output_text","text":"Codex answer"}],"internal_chat_message_metadata_passthrough":{"turn_id":"turn-a"}}}"#,
            )
            .expect("response item");

        assert_eq!(message.turn_id.as_deref(), Some("msg-native"));
        assert_eq!(
            message
                .provider_provenance
                .as_ref()
                .map(|provenance| provenance.provider_turn_id.as_str()),
            Some("turn-a")
        );
    }

    #[test]
    fn codex_watch_keeps_binding_for_agent_message_after_task_complete() {
        let mut state = codex_watch_state();
        let task_started = codex_task_started("turn-a");
        state.observe_record(
            &task_started,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-a".to_string(),
            }),
        );
        let task_complete = codex_task_complete("turn-a");
        state.observe_record(&task_complete, Some(&AgentEvent::TurnCompleted));

        let message = state
            .extract_message(&codex_agent_message("answer after completion"))
            .expect("agent message");

        assert_eq!(
            message
                .provider_provenance
                .as_ref()
                .map(|provenance| provenance.provider_turn_id.as_str()),
            Some("turn-a")
        );
        assert!(state.completed_turn_id.is_none());
    }

    #[test]
    fn codex_watch_rejects_missing_or_replaced_turn_identity() {
        let mut state = codex_watch_state();
        let task_started = codex_task_started("turn-a");
        state.observe_record(
            &task_started,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-a".to_string(),
            }),
        );
        state.observe_record(
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":" "}}"#,
            Some(&AgentEvent::Unknown),
        );
        assert!(state
            .extract_message(&codex_agent_message("stale"))
            .expect("agent message")
            .provider_provenance
            .is_none());

        state.observe_record(
            &task_started,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-a".to_string(),
            }),
        );
        let task_started_b = codex_task_started("turn-b");
        state.observe_record(
            &task_started_b,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-b".to_string(),
            }),
        );
        assert_eq!(
            state
                .extract_message(&codex_agent_message("new turn"))
                .expect("agent message")
                .provider_provenance
                .as_ref()
                .map(|provenance| provenance.provider_turn_id.as_str()),
            Some("turn-b")
        );

        state.observe_record(
            r#"{"type":"response_item","turn_id":"turn-c","payload":{"type":"reasoning","text":"other turn"}}"#,
            Some(&AgentEvent::Generating),
        );
        assert!(state
            .extract_message(&codex_agent_message("after mismatch"))
            .expect("agent message")
            .provider_provenance
            .is_none());
    }

    #[test]
    fn codex_watch_resets_binding_when_source_changes() {
        let mut state = codex_watch_state();
        let task_started = codex_task_started("turn-a");
        state.observe_record(
            &task_started,
            Some(&AgentEvent::TurnStarted {
                turn_id: "turn-a".to_string(),
            }),
        );
        state.set_source("codex-session", "replacement.jsonl");

        assert!(state
            .extract_message(&codex_agent_message("old source"))
            .expect("agent message")
            .provider_provenance
            .is_none());
    }

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
    fn pi_completed_session_message_extracts_assistant_text() {
        let line = r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Pi answer"}],"stopReason":"stop"}}"#;

        let message = extract_transcript_message("pi", line).unwrap();
        assert_eq!(message.text, "Pi answer");
        assert_eq!(message.provider, "pi");
        assert_eq!(message.source.as_deref(), Some("session_jsonl"));
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
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":"wardian reply wf_test --status done --stdin\nprompt echo"}}"#
        )
        .is_none());
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
