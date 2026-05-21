use std::collections::HashSet;
use std::path::Path;

use crate::manager::opencode::opencode_database_path;
use crate::providers::chat_transcript::{normalize_chat_lines, visible_chat_text};
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

    let (watch_state, provider, resume_session, current_status, last_status_at, log_path) = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("agent not found: {session_id}"))?;
        let config = agent
            .config
            .lock()
            .map_err(|_| "agent config lock poisoned".to_string())?;
        let provider = config.provider.clone();
        let resume_session = config.resume_session.clone();
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
        let log_path = agent
            .log_path
            .lock()
            .map_err(|_| "agent log path lock poisoned".to_string())?
            .clone();
        (
            agent.watch_state.clone(),
            provider,
            resume_session,
            current_status,
            last_status_at,
            log_path,
        )
    };

    let snapshot = watch_state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?
        .snapshot_since(None, None)
        .map_err(|error| format!("watch state error: {} {}", error.code(), error.details()))?;

    let mut provider_events =
        load_provider_log_chat_events(&session_id, &provider, log_path.as_deref());
    if provider == "opencode" {
        provider_events.extend(load_opencode_db_chat_events(
            &session_id,
            opencode_session_id(&session_id, resume_session.as_deref()).as_deref(),
        ));
    }
    let provider_has_transcript = has_transcript_events(&provider_events);
    let watch_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
        session_id: &session_id,
        provider: &provider,
        current_status: Some(&current_status),
        last_status_at: last_status_at.as_deref(),
        events: &snapshot.events,
        output: &snapshot.output,
        transcript: &snapshot.transcript,
        include_transcript: !provider_has_transcript,
        include_terminal_output: !provider_has_transcript,
    });

    Ok(merge_chat_events(watch_events, provider_events))
}

struct WatchSnapshotChatInput<'a> {
    session_id: &'a str,
    provider: &'a str,
    current_status: Option<&'a str>,
    last_status_at: Option<&'a str>,
    events: &'a [WatchEvent],
    output: &'a WatchOutput,
    transcript: &'a WatchTranscript,
    include_transcript: bool,
    include_terminal_output: bool,
}

fn map_watch_snapshot_to_chat_events(input: WatchSnapshotChatInput<'_>) -> Vec<AgentChatEvent> {
    let mut sequence = 0_u64;
    let mut chat_events = Vec::new();

    for event in input.events.iter().filter(|event| event.kind == "status") {
        if let Some(status) = event.payload.get("status").and_then(|value| value.as_str()) {
            sequence = sequence.saturating_add(1);
            chat_events.push(status_event_from_watch_event(
                input.session_id,
                input.provider,
                sequence,
                event,
                status,
            ));
        }
    }

    if !chat_events
        .iter()
        .any(|event| event.kind == AgentChatEventKind::Status)
    {
        if let Some(status) = input
            .current_status
            .filter(|status| !status.trim().is_empty())
        {
            sequence = sequence.saturating_add(1);
            chat_events.push(current_status_event(
                input.session_id,
                input.provider,
                sequence,
                status,
                input.last_status_at,
            ));
        }
    }

    if let Some(approval) = approval_event_from_watch_output(
        input.session_id,
        input.provider,
        sequence.saturating_add(1),
        input.current_status,
        input.output,
    ) {
        sequence = sequence.saturating_add(1);
        chat_events.push(approval);
    }

    if input.include_transcript {
        for message in &input.transcript.messages {
            sequence = sequence.saturating_add(1);
            chat_events.push(message_event_from_transcript(
                input.session_id,
                input.provider,
                sequence,
                message,
                input.transcript,
            ));
        }
    }

    if input.include_terminal_output && !input.output.text.trim().is_empty() {
        sequence = sequence.saturating_add(1);
        chat_events.push(terminal_output_event(
            input.session_id,
            input.provider,
            sequence,
            input.output,
        ));
    }

    chat_events
}

fn approval_event_from_watch_output(
    session_id: &str,
    provider: &str,
    sequence: u64,
    current_status: Option<&str>,
    output: &WatchOutput,
) -> Option<AgentChatEvent> {
    let status = current_status.map(normalize_status)?;
    if status != "action_required" {
        return None;
    }

    let text = output.text.trim();
    if text.is_empty() {
        return None;
    }
    if !(text.contains("Do you want to proceed?")
        || text.contains("Requesting permission")
        || text.contains("Identifying Approval Needs"))
    {
        return None;
    }

    let command = approval_command_from_text(text);
    let mut metadata = serde_json::json!({
        "cursor": output.cursor,
        "watch_sequence": sequence_from_cursor(&output.cursor),
    });
    let provider = provider_for_event(None, provider, &mut metadata);

    Some(AgentChatEvent {
        id: event_id(session_id, sequence, "watch_approval"),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::Approval,
        role: None,
        text: command
            .as_ref()
            .map(|command| format!("Requesting permission for:\n{command}"))
            .or_else(|| Some("Approval required in terminal".to_string())),
        title: Some("Approval required".to_string()),
        status: Some(AgentChatStatus::ActionRequired),
        turn_id: None,
        source: Some("watch_output".to_string()),
        command,
        exit_code: None,
        path: None,
        language: Some("shell".to_string()),
        created_at: None,
        sequence: Some(sequence),
        metadata,
    })
}

fn approval_command_from_text(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("Requesting permission for:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .or_else(|| {
            text.lines()
                .find(|line| line.contains("Bash("))
                .map(str::trim)
                .map(ToString::to_string)
        })
}

fn load_provider_log_chat_events(
    session_id: &str,
    provider: &str,
    log_path: Option<&Path>,
) -> Vec<AgentChatEvent> {
    let Some(path) = log_path else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    normalize_chat_lines(session_id, provider, content.lines())
        .into_iter()
        .map(|mut event| {
            set_metadata(&mut event.metadata, "provider_log", true);
            set_metadata(&mut event.metadata, "log_source", "active_agent_log_path");
            event
        })
        .collect()
}

fn load_opencode_db_chat_events(
    wardian_session_id: &str,
    opencode_session_id: Option<&str>,
) -> Vec<AgentChatEvent> {
    let Some(opencode_session_id) = opencode_session_id else {
        return Vec::new();
    };
    let Some(db_path) = opencode_database_path() else {
        return Vec::new();
    };

    load_opencode_db_chat_events_from_db(&db_path, wardian_session_id, opencode_session_id)
        .unwrap_or_default()
}

fn load_opencode_db_chat_events_from_db(
    db_path: &Path,
    wardian_session_id: &str,
    opencode_session_id: &str,
) -> Result<Vec<AgentChatEvent>, String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| err.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.data, p.time_created, m.id, m.data, m.time_created
             FROM part p
             JOIN message m ON m.id = p.message_id
             WHERE p.session_id = ?1 AND m.session_id = ?1
             ORDER BY COALESCE(p.time_created, m.time_created), p.id",
        )
        .map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([opencode_session_id], |row| {
            Ok(OpencodeDbPart {
                part_id: row.get(0)?,
                part_data: row.get(1)?,
                part_time_created: row.get(2)?,
                message_id: row.get(3)?,
                message_data: row.get(4)?,
                message_time_created: row.get(5)?,
            })
        })
        .map_err(|err| err.to_string())?;

    let mut events = Vec::new();
    for row in rows {
        let row = row.map_err(|err| err.to_string())?;
        let Some(event) = opencode_db_part_to_chat_event(
            wardian_session_id,
            opencode_session_id,
            events.len() as u64 + 1,
            row,
        )?
        else {
            continue;
        };
        events.push(event);
    }

    Ok(events)
}

struct OpencodeDbPart {
    part_id: String,
    part_data: String,
    part_time_created: Option<i64>,
    message_id: String,
    message_data: String,
    message_time_created: Option<i64>,
}

fn opencode_db_part_to_chat_event(
    wardian_session_id: &str,
    opencode_session_id: &str,
    sequence: u64,
    row: OpencodeDbPart,
) -> Result<Option<AgentChatEvent>, String> {
    let message: serde_json::Value =
        serde_json::from_str(&row.message_data).map_err(|err| err.to_string())?;
    let part: serde_json::Value =
        serde_json::from_str(&row.part_data).map_err(|err| err.to_string())?;
    if part.get("type").and_then(|value| value.as_str()) != Some("text") {
        return Ok(None);
    }
    let Some(text) = part
        .get("text")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Ok(None);
    };
    let Some(role) = message
        .get("role")
        .and_then(|value| value.as_str())
        .and_then(role_from_str)
    else {
        return Ok(None);
    };

    let Some(text) = visible_chat_text(&role, text) else {
        return Ok(None);
    };

    Ok(Some(AgentChatEvent {
        id: event_id(
            wardian_session_id,
            sequence,
            &format!("opencode_db:{}", row.part_id),
        ),
        session_id: wardian_session_id.to_string(),
        provider: "opencode".to_string(),
        kind: AgentChatEventKind::Message,
        role: Some(role),
        text: Some(text.to_string()),
        title: None,
        status: None,
        turn_id: Some(row.message_id),
        source: Some("opencode_db".to_string()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence: Some(sequence),
        metadata: serde_json::json!({
            "provider_log": true,
            "opencode_session_id": opencode_session_id,
            "part_time_created": row.part_time_created,
            "message_time_created": row.message_time_created,
        }),
    }))
}

fn opencode_session_id(wardian_session_id: &str, resume_session: Option<&str>) -> Option<String> {
    resume_session
        .map(str::trim)
        .filter(|session| session.starts_with("ses_"))
        .or_else(|| {
            wardian_session_id
                .trim()
                .starts_with("ses_")
                .then_some(wardian_session_id)
        })
        .map(ToString::to_string)
}

fn has_transcript_events(events: &[AgentChatEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.kind,
            AgentChatEventKind::Message
                | AgentChatEventKind::ToolCall
                | AgentChatEventKind::ToolResult
                | AgentChatEventKind::Approval
                | AgentChatEventKind::Error
        )
    })
}

fn merge_chat_events(
    watch_events: Vec<AgentChatEvent>,
    provider_events: Vec<AgentChatEvent>,
) -> Vec<AgentChatEvent> {
    let mut seen = HashSet::new();
    let mut merged = Vec::with_capacity(watch_events.len() + provider_events.len());

    for event in provider_events.into_iter().chain(watch_events) {
        let key = chat_event_dedupe_key(&event);
        if seen.insert(key) {
            merged.push(event);
        }
    }

    for (index, event) in merged.iter_mut().enumerate() {
        event.sequence = Some(index as u64 + 1);
    }

    merged
}

fn chat_event_dedupe_key(event: &AgentChatEvent) -> String {
    if event.kind == AgentChatEventKind::Message {
        return format!(
            "{:?}|{:?}|{}|{}",
            event.kind,
            event.role,
            event.turn_id.as_deref().unwrap_or(""),
            event.text.as_deref().unwrap_or("")
        );
    }

    if event.kind == AgentChatEventKind::Status {
        return format!(
            "{:?}|{}|{}",
            event.kind,
            event
                .status
                .as_ref()
                .map(|status| format!("{status:?}"))
                .unwrap_or_default(),
            event.text.as_deref().unwrap_or("")
        );
    }

    format!(
        "{:?}|{:?}|{}|{}|{}|{}|{}",
        event.kind,
        event.role,
        event.turn_id.as_deref().unwrap_or(""),
        event.title.as_deref().unwrap_or(""),
        event.command.as_deref().unwrap_or(""),
        event.text.as_deref().unwrap_or(""),
        event.source.as_deref().unwrap_or("")
    )
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

        let output = output("raw terminal");
        let chat_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
            session_id: "agent-1",
            provider: "codex",
            current_status: Some("Idle"),
            last_status_at: None,
            events: &events,
            output: &output,
            transcript: &transcript,
            include_transcript: true,
            include_terminal_output: true,
        });

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

        let output = output("");
        let chat_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
            session_id: "agent-1",
            provider: "",
            current_status: None,
            last_status_at: None,
            events: &[],
            output: &output,
            transcript: &transcript,
            include_transcript: true,
            include_terminal_output: true,
        });

        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].provider, "unknown");
        assert_eq!(chat_events[0].metadata["provider_source"], "fallback");
        assert_eq!(chat_events[0].metadata["provider_fallback"], true);
    }

    #[test]
    fn adds_current_status_when_watch_snapshot_has_no_status_event() {
        let output = output("");
        let transcript = transcript(Vec::new());
        let chat_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
            session_id: "agent-1",
            provider: "codex",
            current_status: Some("Idle"),
            last_status_at: Some("2026-05-21T00:00:00.000Z"),
            events: &[],
            output: &output,
            transcript: &transcript,
            include_transcript: true,
            include_terminal_output: true,
        });

        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].kind, AgentChatEventKind::Status);
        assert_eq!(chat_events[0].status, Some(AgentChatStatus::Idle));
        assert_eq!(
            chat_events[0].created_at.as_deref(),
            Some("2026-05-21T00:00:00.000Z")
        );
    }

    #[test]
    fn maps_action_needed_terminal_prompt_to_approval_event() {
        let output = output(
            r#"Identifying Approval Needs

Bash(Get-ChildItem -Path "C:\Users\tgemi\AppData\Local\Temp\wardian-antigravity\include")

Command

Requesting permission for: Get-ChildItem -Path "C:\Users\tgemi\AppData\Local\Temp\wardian-antigravity\include"

Do you want to proceed?
> 1. Yes"#,
        );
        let transcript = transcript(vec![WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "Prior answer".to_string(),
            provider: "antigravity".to_string(),
            turn_id: Some("turn-1".to_string()),
            source: Some("transcript".to_string()),
        }]);

        let chat_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
            session_id: "agent-1",
            provider: "antigravity",
            current_status: Some("Action Needed"),
            last_status_at: None,
            events: &[],
            output: &output,
            transcript: &transcript,
            include_transcript: false,
            include_terminal_output: false,
        });

        let approval = chat_events
            .iter()
            .find(|event| event.kind == AgentChatEventKind::Approval)
            .expect("approval event");
        assert_eq!(approval.status, Some(AgentChatStatus::ActionRequired));
        assert!(approval
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("Get-ChildItem -Path"));
    }

    #[test]
    fn loads_provider_log_events_from_active_agent_log_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        std::fs::write(
            &log_path,
            r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"user","content":"Try the chat view"}}"#
                .to_string()
                + "\n"
                + r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Rendered from the provider log"}]}}"#,
        )
        .expect("write log");

        let chat_events = load_provider_log_chat_events("agent-1", "codex", Some(&log_path));

        assert_eq!(chat_events.len(), 2);
        assert_eq!(chat_events[0].kind, AgentChatEventKind::Message);
        assert_eq!(chat_events[0].role, Some(AgentChatRole::User));
        assert_eq!(chat_events[1].role, Some(AgentChatRole::Assistant));
        assert_eq!(
            chat_events[1].text.as_deref(),
            Some("Rendered from the provider log")
        );
        assert_eq!(chat_events[0].metadata["provider_log"], true);
    }

    #[test]
    fn provider_log_transcript_suppresses_watch_terminal_fallback() {
        let provider_events = vec![AgentChatEvent {
            id: "agent-1:provider:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some("Structured answer".to_string()),
            title: None,
            status: None,
            turn_id: Some("turn-1".to_string()),
            source: Some("response_item".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({}),
        }];

        let include_watch_fallback = !has_transcript_events(&provider_events);
        let output = output("raw terminal fallback");
        let transcript = transcript(Vec::new());
        let watch_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
            session_id: "agent-1",
            provider: "codex",
            current_status: Some("Idle"),
            last_status_at: None,
            events: &[],
            output: &output,
            transcript: &transcript,
            include_transcript: include_watch_fallback,
            include_terminal_output: include_watch_fallback,
        });
        let chat_events = merge_chat_events(watch_events, provider_events);

        assert!(chat_events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::Message));
        assert!(!chat_events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::TerminalOutput));
        assert!(chat_events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::Status));
    }

    #[test]
    fn merge_places_current_watch_approval_after_provider_transcript() {
        let provider_events = vec![AgentChatEvent {
            id: "agent-1:provider:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some("Earlier structured answer".to_string()),
            title: None,
            status: None,
            turn_id: Some("turn-1".to_string()),
            source: Some("response_item".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({}),
        }];
        let output = output("Requesting permission for: npm test\nDo you want to proceed?");
        let transcript = transcript(Vec::new());
        let watch_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
            session_id: "agent-1",
            provider: "codex",
            current_status: Some("Action Required"),
            last_status_at: None,
            events: &[],
            output: &output,
            transcript: &transcript,
            include_transcript: false,
            include_terminal_output: false,
        });

        let chat_events = merge_chat_events(watch_events, provider_events);

        assert_eq!(chat_events[0].kind, AgentChatEventKind::Message);
        assert_eq!(chat_events[1].kind, AgentChatEventKind::Status);
        assert_eq!(chat_events[2].kind, AgentChatEventKind::Approval);
        assert_eq!(chat_events[2].status, Some(AgentChatStatus::ActionRequired));
    }

    #[test]
    fn merge_deduplicates_repeated_message_records_from_distinct_sources() {
        let first = AgentChatEvent {
            id: "agent-1:provider:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some("Same answer".to_string()),
            title: None,
            status: None,
            turn_id: Some("turn-1".to_string()),
            source: Some("response_item".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({}),
        };
        let mut duplicate = first.clone();
        duplicate.id = "agent-1:provider:2".to_string();
        duplicate.source = Some("item.completed".to_string());

        let chat_events = merge_chat_events(Vec::new(), vec![first, duplicate]);

        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].text.as_deref(), Some("Same answer"));
    }

    #[test]
    fn opencode_db_text_parts_are_loaded_as_user_and_agent_messages() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db_path = temp.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE message (
                id text PRIMARY KEY,
                session_id text NOT NULL,
                time_created integer,
                time_updated integer,
                data text NOT NULL
            );
            CREATE TABLE part (
                id text PRIMARY KEY,
                message_id text NOT NULL,
                session_id text NOT NULL,
                time_created integer,
                time_updated integer,
                data text NOT NULL
            );
            INSERT INTO message VALUES ('msg-user', 'ses_test', 1, 1, '{"role":"user"}');
            INSERT INTO part VALUES ('part-user', 'msg-user', 'ses_test', 2, 2, '{"type":"text","text":"List 50 numbers."}');
            INSERT INTO message VALUES ('msg-assistant', 'ses_test', 3, 3, '{"role":"assistant"}');
            INSERT INTO part VALUES ('part-finish', 'msg-assistant', 'ses_test', 4, 4, '{"type":"finish","reason":"stop"}');
            INSERT INTO part VALUES ('part-assistant', 'msg-assistant', 'ses_test', 5, 5, '{"type":"text","text":"1, 2, 3"}');
            "#,
        )
        .expect("seed db");

        let chat_events =
            load_opencode_db_chat_events_from_db(&db_path, "agent-1", "ses_test").expect("load db");

        assert_eq!(chat_events.len(), 2);
        assert_eq!(chat_events[0].provider, "opencode");
        assert_eq!(chat_events[0].role, Some(AgentChatRole::User));
        assert_eq!(chat_events[0].text.as_deref(), Some("List 50 numbers."));
        assert_eq!(chat_events[1].role, Some(AgentChatRole::Assistant));
        assert_eq!(chat_events[1].text.as_deref(), Some("1, 2, 3"));
        assert_eq!(chat_events[1].source.as_deref(), Some("opencode_db"));
    }

    #[test]
    fn opencode_session_id_prefers_real_resume_session() {
        assert_eq!(
            opencode_session_id("wardian-uuid", Some("ses_real")).as_deref(),
            Some("ses_real")
        );
        assert_eq!(
            opencode_session_id("ses_from_agent", None).as_deref(),
            Some("ses_from_agent")
        );
        assert_eq!(
            opencode_session_id("wardian-uuid", Some("stale-uuid")),
            None
        );
    }
}
