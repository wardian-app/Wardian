use sha2::{Digest, Sha256};
use wardian_core::control::{
    WatchMirrorIdentity, WatchTranscriptMessage, WatchTranscriptProvenance,
};
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
                        mirror: None,
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

pub(crate) fn bind_pi_watch_message(
    message: &mut WatchTranscriptMessage,
    provider_session_id: &str,
    source_path: &str,
) {
    let provider_session_id = provider_session_id.trim();
    let source_path = source_path.trim();
    let provider_turn_id = message.turn_id.as_deref().map(str::trim).unwrap_or("");
    if !message.provider.eq_ignore_ascii_case("pi")
        || message.source.as_deref() != Some("session_jsonl")
        || provider_session_id.is_empty()
        || source_path.is_empty()
        || provider_turn_id.is_empty()
    {
        return;
    }
    message.provider_provenance = Some(WatchTranscriptProvenance {
        provider_session_id: provider_session_id.to_string(),
        source_path: source_path.to_string(),
        provider_turn_id: provider_turn_id.to_string(),
        mirror: None,
    });
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
    let turn_id = if kind == "message" {
        parsed.get("id").or_else(|| message.get("id"))
    } else {
        message.get("id")
    }
    .and_then(|value| value.as_str())
    .map(str::to_string);
    Some(WatchTranscriptMessage {
        role: "assistant".into(),
        text,
        provider: "pi".into(),
        turn_id,
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
        provider_provenance: mock_mirror_provenance(&parsed),
    })
}

/// Preserve a well-shaped producer claim without trusting any JSON source path.
/// The digest covers the parsed raw object, never a producer-supplied digest.
pub(crate) fn mock_mirror_provenance(
    parsed: &serde_json::Value,
) -> Option<WatchTranscriptProvenance> {
    let receipt = parsed.get("wardian_mirror")?.as_object()?;
    let observation_id = receipt.get("observation_id")?.as_str()?;
    let incarnation = receipt.get("incarnation")?.as_str()?;
    let provider_session_id = receipt.get("provider_session_id")?.as_str()?;
    if !valid_mock_mirror_identity(observation_id, incarnation)
        || !valid_mock_receipt_id(provider_session_id)
    {
        return None;
    }
    Some(WatchTranscriptProvenance {
        provider_session_id: provider_session_id.to_string(),
        source_path: String::new(),
        provider_turn_id: parsed
            .get("turn_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        mirror: Some(WatchMirrorIdentity {
            observation_id: observation_id.to_string(),
            incarnation: incarnation.to_string(),
            logged: receipt.get("logged")?.as_bool()?,
            payload_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(parsed).ok()?)),
        }),
    })
}

fn valid_mock_receipt_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_mock_mirror_identity(observation_id: &str, incarnation: &str) -> bool {
    if !valid_mock_receipt_id(observation_id) || !valid_mock_receipt_id(incarnation) {
        return false;
    }
    let Ok(uuid) = uuid::Uuid::parse_str(incarnation) else {
        return false;
    };
    if uuid.is_nil() || uuid.to_string() != incarnation {
        return false;
    }
    let Some(sequence) = observation_id
        .strip_prefix(incarnation)
        .and_then(|suffix| suffix.strip_prefix(':'))
    else {
        return false;
    };
    sequence
        .parse::<u64>()
        .is_ok_and(|number| number > 0 && number.to_string() == sequence)
}

/// Bind a logged Mock claim to the callback's actual session and host-owned path.
/// This remains a Watch observation until an acquired record verifies the claim.
pub(crate) fn bind_mock_watch_message(
    message: &mut WatchTranscriptMessage,
    expected_provider_session: &str,
    source_path: &str,
) {
    if !message.provider.eq_ignore_ascii_case("mock") {
        return;
    }
    let Some(provenance) = message.provider_provenance.as_mut() else {
        return;
    };
    // A rejected claim must not retain an earlier or JSON-supplied path.
    provenance.source_path.clear();
    let Some(mirror) = provenance.mirror.as_ref() else {
        return;
    };
    if !mirror.logged
        || !valid_mock_mirror_identity(&mirror.observation_id, &mirror.incarnation)
        || mirror.payload_sha256.len() != 64
        || !mirror
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !valid_mock_receipt_id(expected_provider_session)
        || provenance.provider_session_id != expected_provider_session
        || source_path.trim().is_empty()
    {
        return;
    }
    provenance.source_path = std::fs::canonicalize(source_path)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| source_path.to_string());
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
                mirror: None,
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

    fn mock_observation() -> serde_json::Value {
        serde_json::json!({
            "type": "model", "content": "Mock answer", "turn_id": "turn-7",
            "wardian_mirror": {
                "observation_id": "c60e52bf-71f2-43f1-a313-9ea96fe2f7bc:1",
                "incarnation": "c60e52bf-71f2-43f1-a313-9ea96fe2f7bc",
                "provider_session_id": "mock-session", "logged": true
            }
        })
    }

    #[test]
    fn mock_receipts_preserve_claims_without_trusting_paths_or_digests() {
        for kind in ["message", "model", "info"] {
            let mut raw = mock_observation();
            raw["type"] = serde_json::json!(kind);
            raw["role"] = serde_json::json!("assistant");
            raw["source_path"] = serde_json::json!("forged.jsonl");
            raw["wardian_mirror"]["source_path"] = serde_json::json!("also-forged.jsonl");
            raw["wardian_mirror"]["payload_sha256"] = serde_json::json!("untrusted digest");
            let message = extract_transcript_message("mock", &raw.to_string()).unwrap();
            assert_eq!(message.text, "Mock answer");
            assert_eq!(message.turn_id.as_deref(), Some("turn-7"));
            assert_eq!(message.source.as_deref(), Some(kind));
            let provenance = message.provider_provenance.unwrap();
            assert_eq!(provenance.provider_session_id, "mock-session");
            assert_eq!(provenance.provider_turn_id, "turn-7");
            assert!(provenance.source_path.is_empty());
            let mirror = provenance.mirror.unwrap();
            assert!(mirror.logged);
            assert_eq!(
                mirror.observation_id,
                raw["wardian_mirror"]["observation_id"].as_str().unwrap()
            );
            assert_eq!(
                mirror.payload_sha256,
                format!("{:x}", Sha256::digest(serde_json::to_vec(&raw).unwrap()))
            );

            let mut conflicting = raw.clone();
            conflicting["content"] = serde_json::json!("Different raw payload");
            let conflict = mock_mirror_provenance(&conflicting)
                .unwrap()
                .mirror
                .unwrap();
            assert_eq!(conflict.observation_id, mirror.observation_id);
            assert_ne!(conflict.payload_sha256, mirror.payload_sha256);
            // Object key order and JSON whitespace do not define the digest.
            let reordered: serde_json::Value =
                serde_json::from_str(&serde_json::to_string_pretty(&raw).unwrap()).unwrap();
            assert_eq!(
                mock_mirror_provenance(&reordered),
                mock_mirror_provenance(&raw)
            );
        }
        let mut without_turn = mock_observation();
        without_turn.as_object_mut().unwrap().remove("turn_id");
        without_turn["id"] = serde_json::json!("legacy-message-id");
        assert_eq!(
            mock_mirror_provenance(&without_turn)
                .unwrap()
                .provider_turn_id,
            ""
        );
        assert_eq!(
            extract_transcript_message("mock", &without_turn.to_string())
                .unwrap()
                .turn_id
                .as_deref(),
            Some("legacy-message-id")
        );
    }

    #[test]
    fn mock_missing_or_malformed_receipts_leave_genuine_output_unproved() {
        let mut absent = mock_observation();
        absent.as_object_mut().unwrap().remove("wardian_mirror");
        let message = extract_transcript_message("mock", &absent.to_string()).unwrap();
        assert!(message.provider_provenance.is_none());
        assert_eq!(message.text, "Mock answer");
        for (key, value) in [
            ("observation_id", serde_json::json!("")),
            ("observation_id", serde_json::json!("x".repeat(129))),
            ("observation_id", serde_json::json!("other-incarnation:1")),
            (
                "observation_id",
                serde_json::json!("c60e52bf-71f2-43f1-a313-9ea96fe2f7bc:0"),
            ),
            (
                "observation_id",
                serde_json::json!("c60e52bf-71f2-43f1-a313-9ea96fe2f7bc:01"),
            ),
            ("incarnation", serde_json::json!("not-a-uuid")),
            (
                "incarnation",
                serde_json::json!("c60e52bf-71f2-43f1-a313-9ea96fe2f7bd"),
            ),
            ("provider_session_id", serde_json::json!("")),
            ("provider_session_id", serde_json::json!("x".repeat(129))),
            ("provider_session_id", serde_json::json!("mock session\n")),
            ("provider_session_id", serde_json::json!(7)),
            ("logged", serde_json::json!("true")),
            ("logged", serde_json::Value::Null),
        ] {
            let mut raw = mock_observation();
            raw["wardian_mirror"][key] = value;
            let message = extract_transcript_message("mock", &raw.to_string()).unwrap();
            assert!(message.provider_provenance.is_none(), "accepted {raw}");
            assert_eq!(message.text, "Mock answer");
        }
        for key in [
            "observation_id",
            "incarnation",
            "provider_session_id",
            "logged",
        ] {
            let mut raw = mock_observation();
            raw["wardian_mirror"].as_object_mut().unwrap().remove(key);
            assert!(mock_mirror_provenance(&raw).is_none());
        }
        for receipt in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!(true),
        ] {
            let mut raw = mock_observation();
            raw["wardian_mirror"] = receipt;
            assert!(mock_mirror_provenance(&raw).is_none());
        }
        let mut boundary = mock_observation();
        boundary["wardian_mirror"]["provider_session_id"] = serde_json::json!("x".repeat(128));
        assert!(mock_mirror_provenance(&boundary).is_some());
    }

    #[test]
    fn mock_binding_requires_logged_claim_exact_session_and_host_path() {
        let raw = mock_observation();
        let original = extract_transcript_message("mock", &raw.to_string()).unwrap();
        let host_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/providers/transcript.rs");
        let canonical = std::fs::canonicalize(&host_path).unwrap();
        let mut message = original.clone();
        bind_mock_watch_message(&mut message, "mock-session", host_path.to_str().unwrap());
        assert_eq!(
            message.provider_provenance.as_ref().unwrap().source_path,
            canonical.to_string_lossy().as_ref()
        );
        assert_eq!(message.turn_id, original.turn_id);
        assert_eq!(message.source, original.source);
        assert_eq!(message.text, original.text);

        let mut unavailable = original.clone();
        let unavailable_path = host_path.with_extension("missing-mock-mirror");
        bind_mock_watch_message(
            &mut unavailable,
            "mock-session",
            unavailable_path.to_str().unwrap(),
        );
        assert_eq!(
            unavailable.provider_provenance.unwrap().source_path,
            unavailable_path.to_str().unwrap()
        );
        let mut missing_mirror = original.clone();
        missing_mirror.provider_provenance.as_mut().unwrap().mirror = None;
        bind_mock_watch_message(&mut missing_mirror, "mock-session", "host.jsonl");
        assert!(missing_mirror
            .provider_provenance
            .unwrap()
            .source_path
            .is_empty());

        for (session, path) in [
            ("another-session", "host.jsonl"),
            ("", "host.jsonl"),
            ("mock-session", " "),
        ] {
            let mut rejected = message.clone();
            bind_mock_watch_message(&mut rejected, session, path);
            assert!(rejected.provider_provenance.unwrap().source_path.is_empty());
        }
        for field in ["logged", "observation_id", "incarnation", "payload_sha256"] {
            let mut rejected = original.clone();
            let mirror = rejected
                .provider_provenance
                .as_mut()
                .unwrap()
                .mirror
                .as_mut()
                .unwrap();
            match field {
                "logged" => mirror.logged = false,
                "observation_id" => mirror.observation_id = "wrong:1".to_string(),
                "incarnation" => mirror.incarnation = "malformed".to_string(),
                _ => mirror.payload_sha256 = "unproved".to_string(),
            }
            bind_mock_watch_message(&mut rejected, "mock-session", "host.jsonl");
            assert!(rejected.provider_provenance.unwrap().source_path.is_empty());
        }
        let mut unlogged = raw.clone();
        unlogged["wardian_mirror"]["logged"] = serde_json::json!(false);
        let mut unlogged = extract_transcript_message("mock", &unlogged.to_string()).unwrap();
        bind_mock_watch_message(&mut unlogged, "mock-session", "host.jsonl");
        assert!(unlogged.provider_provenance.unwrap().source_path.is_empty());
        let mut other_provider = message.clone();
        other_provider.provider = "pi".to_string();
        let unchanged = other_provider.clone();
        bind_mock_watch_message(&mut other_provider, "mock-session", "host.jsonl");
        assert_eq!(other_provider, unchanged);
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
        let line = r#"{"type":"message","id":"pi-entry","message":{"role":"assistant","content":[{"type":"text","text":"Pi answer"}],"stopReason":"stop"}}"#;

        let message = extract_transcript_message("pi", line).unwrap();
        assert_eq!(message.text, "Pi answer");
        assert_eq!(message.provider, "pi");
        assert_eq!(message.turn_id.as_deref(), Some("pi-entry"));
        assert_eq!(message.source.as_deref(), Some("session_jsonl"));
    }

    #[test]
    fn pi_watch_message_preserves_launch_owned_source_binding() {
        let mut message = extract_transcript_message(
            "pi",
            r#"{"type":"message","id":"pi-entry","message":{"role":"assistant","content":[{"type":"text","text":"Pi answer"}],"stopReason":"stop"}}"#,
        )
        .unwrap();

        bind_pi_watch_message(&mut message, "pi-session", "pi-session.jsonl");

        assert_eq!(
            message.provider_provenance,
            Some(WatchTranscriptProvenance {
                provider_session_id: "pi-session".to_string(),
                source_path: "pi-session.jsonl".to_string(),
                provider_turn_id: "pi-entry".to_string(),
                mirror: None,
            })
        );

        let message_end = extract_transcript_message(
            "pi",
            r#"{"type":"message_end","id":"outer-end","message":{"role":"assistant","id":"nested-end","content":"Pi answer","stopReason":"stop"}}"#,
        )
        .unwrap();
        assert_eq!(message_end.turn_id.as_deref(), Some("nested-end"));
        assert_eq!(message_end.source.as_deref(), Some("json_mode"));
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
