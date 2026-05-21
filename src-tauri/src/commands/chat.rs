use crate::state::AppState;
use tauri::State;
use wardian_core::control::{WatchEvent, WatchOutput, WatchTranscript, WatchTranscriptMessage};
use wardian_core::identity::normalize_status;
use wardian_core::models::{AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus};

#[tauri::command]
pub async fn load_agent_chat_transcript(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<AgentChatEvent>, String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("session_id is required".to_string());
    }

    let (watch_state, provider, current_status, last_status_at) = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("agent not found: {session_id}"))?;
        let provider = agent
            .config
            .lock()
            .map_err(|_| "agent config lock poisoned".to_string())?
            .provider
            .clone();
        let current_status = agent
            .current_status
            .lock()
            .map_err(|_| "agent status lock poisoned".to_string())?
            .clone();
        let last_status_at = agent
            .last_status_at
            .lock()
            .map_err(|_| "agent status timestamp lock poisoned".to_string())?
            .clone();
        (
            agent.watch_state.clone(),
            provider,
            current_status,
            last_status_at,
        )
    };

    let snapshot = watch_state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?
        .snapshot_since(None, None)
        .map_err(|error| format!("watch state error: {} {}", error.code(), error.details()))?;

    Ok(map_watch_snapshot_to_chat_events(
        &session_id,
        &provider,
        Some(&current_status),
        last_status_at.as_deref(),
        &snapshot.events,
        &snapshot.output,
        &snapshot.transcript,
    ))
}

fn map_watch_snapshot_to_chat_events(
    session_id: &str,
    provider: &str,
    current_status: Option<&str>,
    last_status_at: Option<&str>,
    events: &[WatchEvent],
    output: &WatchOutput,
    transcript: &WatchTranscript,
) -> Vec<AgentChatEvent> {
    let mut sequence = 0_u64;
    let mut chat_events = Vec::new();

    for event in events.iter().filter(|event| event.kind == "status") {
        if let Some(status) = event.payload.get("status").and_then(|value| value.as_str()) {
            sequence = sequence.saturating_add(1);
            chat_events.push(status_event_from_watch_event(
                session_id, provider, sequence, event, status,
            ));
        }
    }

    if !chat_events
        .iter()
        .any(|event| event.kind == AgentChatEventKind::Status)
    {
        if let Some(status) = current_status.filter(|status| !status.trim().is_empty()) {
            sequence = sequence.saturating_add(1);
            chat_events.push(current_status_event(
                session_id,
                provider,
                sequence,
                status,
                last_status_at,
            ));
        }
    }

    for message in &transcript.messages {
        sequence = sequence.saturating_add(1);
        chat_events.push(message_event_from_transcript(
            session_id, provider, sequence, message, transcript,
        ));
    }

    if !output.text.trim().is_empty() {
        sequence = sequence.saturating_add(1);
        chat_events.push(terminal_output_event(
            session_id, provider, sequence, output,
        ));
    }

    chat_events
}

fn status_event_from_watch_event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    event: &WatchEvent,
    raw_status: &str,
) -> AgentChatEvent {
    let created_at = event
        .payload
        .get("observed_at")
        .and_then(|value| value.as_str())
        .map(ToString::to_string);
    let mut metadata = serde_json::json!({
        "cursor": event.cursor,
        "payload": event.payload,
        "raw_status": raw_status,
        "watch_sequence": sequence_from_cursor(&event.cursor),
    });
    let provider = provider_for_event(None, provider, &mut metadata);
    let status = chat_status_from_str(raw_status);

    AgentChatEvent {
        id: event_id(session_id, sequence, "watch_status"),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::Status,
        role: None,
        text: Some(normalize_status(raw_status)),
        title: Some("Status".to_string()),
        status: Some(status),
        turn_id: None,
        source: Some("watch_status".to_string()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at,
        sequence: Some(sequence),
        metadata,
    }
}

fn current_status_event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    raw_status: &str,
    last_status_at: Option<&str>,
) -> AgentChatEvent {
    let mut metadata = serde_json::json!({
        "raw_status": raw_status,
        "snapshot": "current_agent_status",
    });
    let provider = provider_for_event(None, provider, &mut metadata);
    let status = chat_status_from_str(raw_status);

    AgentChatEvent {
        id: event_id(session_id, sequence, "current_status"),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::Status,
        role: None,
        text: Some(normalize_status(raw_status)),
        title: Some("Status".to_string()),
        status: Some(status),
        turn_id: None,
        source: Some("current_status".to_string()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: last_status_at.map(ToString::to_string),
        sequence: Some(sequence),
        metadata,
    }
}

fn message_event_from_transcript(
    session_id: &str,
    state_provider: &str,
    sequence: u64,
    message: &WatchTranscriptMessage,
    transcript: &WatchTranscript,
) -> AgentChatEvent {
    let mut metadata = serde_json::json!({
        "transcript_cursor": transcript.cursor,
        "transcript_truncated": transcript.truncated,
        "transcript_omitted_bytes": transcript.omitted_bytes,
        "raw_role": message.role,
    });
    let provider = provider_for_event(Some(&message.provider), state_provider, &mut metadata);
    let role = role_from_str(&message.role);

    AgentChatEvent {
        id: event_id(
            session_id,
            sequence,
            message.source.as_deref().unwrap_or("transcript"),
        ),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::Message,
        role,
        text: Some(message.text.clone()),
        title: None,
        status: None,
        turn_id: message.turn_id.clone(),
        source: message
            .source
            .clone()
            .or_else(|| Some("transcript".to_string())),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence: Some(sequence),
        metadata,
    }
}

fn terminal_output_event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    output: &WatchOutput,
) -> AgentChatEvent {
    let mut metadata = serde_json::json!({
        "cursor": output.cursor,
        "truncated": output.truncated,
        "omitted_bytes": output.omitted_bytes,
        "watch_sequence": sequence_from_cursor(&output.cursor),
    });
    let provider = provider_for_event(None, provider, &mut metadata);

    AgentChatEvent {
        id: event_id(session_id, sequence, "terminal_output"),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::TerminalOutput,
        role: None,
        text: Some(output.text.clone()),
        title: Some("Terminal output".to_string()),
        status: None,
        turn_id: None,
        source: Some("watch_output".to_string()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence: Some(sequence),
        metadata,
    }
}

fn provider_for_event(
    event_provider: Option<&str>,
    state_provider: &str,
    metadata: &mut serde_json::Value,
) -> String {
    if let Some(provider) = event_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    {
        set_metadata(metadata, "provider_source", "event");
        return provider.to_string();
    }

    let state_provider = state_provider.trim();
    if !state_provider.is_empty() {
        set_metadata(metadata, "provider_source", "agent_config");
        return state_provider.to_string();
    }

    set_metadata(metadata, "provider_source", "fallback");
    set_metadata(metadata, "provider_fallback", true);
    set_metadata(
        metadata,
        "provider_fallback_reason",
        "provider unavailable in watch transcript and agent config",
    );
    "unknown".to_string()
}

fn set_metadata(metadata: &mut serde_json::Value, key: &str, value: impl Into<serde_json::Value>) {
    if let Some(object) = metadata.as_object_mut() {
        object.insert(key.to_string(), value.into());
    }
}

fn role_from_str(role: &str) -> Option<AgentChatRole> {
    match role.trim().to_ascii_lowercase().as_str() {
        "user" => Some(AgentChatRole::User),
        "assistant" => Some(AgentChatRole::Assistant),
        "system" => Some(AgentChatRole::System),
        "tool" => Some(AgentChatRole::Tool),
        _ => None,
    }
}

fn chat_status_from_str(status: &str) -> AgentChatStatus {
    match normalize_status(status).as_str() {
        "idle" => AgentChatStatus::Idle,
        "processing" => AgentChatStatus::Processing,
        "action_required" => AgentChatStatus::ActionRequired,
        "running" | "headless" => AgentChatStatus::Running,
        "succeeded" | "success" | "done" | "completed" => AgentChatStatus::Succeeded,
        "failed" | "failure" | "error" => AgentChatStatus::Failed,
        "cancelled" | "canceled" | "off" => AgentChatStatus::Cancelled,
        _ => AgentChatStatus::Unknown,
    }
}

fn sequence_from_cursor(cursor: &str) -> Option<u64> {
    cursor
        .rsplit_once(':')
        .and_then(|(_, sequence)| u64::from_str_radix(sequence, 16).ok())
}

fn event_id(session_id: &str, sequence: u64, source: &str) -> String {
    format!("{session_id}:{sequence:016x}:{source}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(text: &str) -> WatchOutput {
        WatchOutput {
            cursor: "agent-1:0000000000000003".to_string(),
            text: text.to_string(),
            truncated: false,
            omitted_bytes: 0,
        }
    }

    fn transcript(messages: Vec<WatchTranscriptMessage>) -> WatchTranscript {
        WatchTranscript {
            cursor: "agent-1:0000000000000002".to_string(),
            messages,
            latest_text: "hello".to_string(),
            truncated: false,
            omitted_bytes: 0,
        }
    }

    #[test]
    fn maps_watch_status_transcript_and_terminal_output() {
        let events = vec![WatchEvent {
            cursor: "agent-1:0000000000000001".to_string(),
            kind: "status".to_string(),
            payload: serde_json::json!({
                "status": "Processing...",
                "observed_at": "2026-05-21T00:00:00.000Z",
            }),
        }];
        let transcript = transcript(vec![WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "hello".to_string(),
            provider: "mock".to_string(),
            turn_id: Some("turn-1".to_string()),
            source: Some("transcript".to_string()),
        }]);

        let chat_events = map_watch_snapshot_to_chat_events(
            "agent-1",
            "codex",
            Some("Idle"),
            None,
            &events,
            &output("raw terminal"),
            &transcript,
        );

        assert_eq!(chat_events.len(), 3);
        assert_eq!(chat_events[0].kind, AgentChatEventKind::Status);
        assert_eq!(chat_events[0].status, Some(AgentChatStatus::Processing));
        assert_eq!(chat_events[1].kind, AgentChatEventKind::Message);
        assert_eq!(chat_events[1].provider, "mock");
        assert_eq!(chat_events[1].role, Some(AgentChatRole::Assistant));
        assert_eq!(chat_events[2].kind, AgentChatEventKind::TerminalOutput);
        assert!(chat_events
            .iter()
            .all(|event| event.id.starts_with("agent-1:")));
    }

    #[test]
    fn falls_back_to_unknown_provider_with_metadata_when_provider_is_unavailable() {
        let transcript = transcript(vec![WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "hello".to_string(),
            provider: String::new(),
            turn_id: None,
            source: None,
        }]);

        let chat_events = map_watch_snapshot_to_chat_events(
            "agent-1",
            "",
            None,
            None,
            &[],
            &output(""),
            &transcript,
        );

        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].provider, "unknown");
        assert_eq!(chat_events[0].metadata["provider_source"], "fallback");
        assert_eq!(chat_events[0].metadata["provider_fallback"], true);
    }

    #[test]
    fn adds_current_status_when_watch_snapshot_has_no_status_event() {
        let chat_events = map_watch_snapshot_to_chat_events(
            "agent-1",
            "codex",
            Some("Idle"),
            Some("2026-05-21T00:00:00.000Z"),
            &[],
            &output(""),
            &transcript(Vec::new()),
        );

        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].kind, AgentChatEventKind::Status);
        assert_eq!(chat_events[0].status, Some(AgentChatStatus::Idle));
        assert_eq!(
            chat_events[0].created_at.as_deref(),
            Some("2026-05-21T00:00:00.000Z")
        );
    }
}
