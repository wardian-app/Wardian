use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::manager::{self, opencode::opencode_database_path};
use crate::providers::antigravity::AntigravityProvider;
use crate::providers::chat_transcript::{
    legacy_visible_chat_text_for_provider, normalize_chat_lines, visible_chat_text,
    visible_chat_text_for_provider,
};
use crate::providers::pi::PiProvider;
use crate::state::conversation_archive::{
    effective_conversation_logging, ConversationArchiveContext,
};
use crate::state::{AgentWatchState, AppState};
use sha2::{Digest, Sha256};
use tauri::State;
use wardian_core::control::{WatchEvent, WatchOutput, WatchTranscript, WatchTranscriptMessage};
use wardian_core::conversations::{AgentConversationLoggingSetting, ConversationLoggingSetting};
use wardian_core::identity::normalize_status;
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

const PROVIDER_LOG_TAIL_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AgentArchiveCaptureSnapshot {
    pub(crate) session_id: String,
    pub(crate) provider: String,
    pub(crate) resume_session: Option<String>,
    pub(crate) fresh_provider_session_id: Option<String>,
    pub(crate) cleared_provider_sessions: Vec<String>,
    pub(crate) current_status: String,
    pub(crate) last_status_at: Option<String>,
    pub(crate) log_path: Option<PathBuf>,
    pub(crate) agent_name: String,
    pub(crate) agent_class: String,
    pub(crate) workspace: String,
    pub(crate) agent_conversation_logging: AgentConversationLoggingSetting,
    pub(crate) watch_state: Arc<Mutex<AgentWatchState>>,
}

pub(crate) struct ArchiveCaptureResult {
    pub(crate) events: Vec<AgentChatEvent>,
    pub(crate) context: ConversationArchiveContext,
}

#[tauri::command]
pub async fn load_agent_chat_transcript(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<AgentChatEvent>, String> {
    load_agent_chat_transcript_for_state(&state, session_id).await
}

pub async fn load_agent_chat_transcript_for_state(
    state: &AppState,
    session_id: String,
) -> Result<Vec<AgentChatEvent>, String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("session_id is required".to_string());
    }

    let result = archive_agent_chat_events_for_state(state, &session_id).await?;
    let archived_events = state
        .conversation_archive
        .chat_events_for_active_conversation(&session_id)
        .unwrap_or_else(|error| {
            manager::log_debug(&format!(
                "[WARDIAN] conversation archive chat replay failed for {session_id}: {error}"
            ));
            Vec::new()
        });

    // Provider logs and the watch snapshot are live, bounded sources. Replay
    // only the active durable archive so a restart or log rotation does not
    // erase current chat rows, while a new provider session starts empty.
    let mut events = merge_chat_events(result.events, archived_events);
    let conversation_started_at = active_conversation_started_at(state, &session_id);
    events.extend(memory_chat_events(
        &session_id,
        conversation_started_at.as_deref(),
    ));
    sort_chat_events(&mut events);
    Ok(events)
}

fn active_conversation_started_at(state: &AppState, session_id: &str) -> Option<String> {
    if let Ok(Some(started_at)) = state
        .conversation_archive
        .live_conversation_started_at(session_id)
    {
        return Some(started_at);
    }
    let conversation_id = state
        .conversation_archive
        .active_conversation_id(session_id)
        .ok()??;
    state
        .conversation_archive
        .list(Some(session_id), false)
        .ok()?
        .into_iter()
        .find(|entry| entry.conversation_id == conversation_id)
        .map(|entry| entry.started_at)
}

fn memory_chat_events(
    session_id: &str,
    conversation_started_at: Option<&str>,
) -> Vec<AgentChatEvent> {
    let Ok(store) = wardian_core::memory::MemoryStore::from_default_home() else {
        return Vec::new();
    };
    let Ok(events) = store.list_events(&wardian_core::memory::MemoryActor::Operator, session_id)
    else {
        return Vec::new();
    };
    events
        .into_iter()
        .filter(|event| memory_event_belongs_to_conversation(event, conversation_started_at))
        .map(|event| {
            let (title, text) = match event.action.as_str() {
                "saved" => (
                    "Memory saved · This agent",
                    event
                        .payload
                        .get("evidence_excerpt")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                ),
                "updated" => (
                    "Memory updated · This agent",
                    event
                        .payload
                        .get("evidence_excerpt")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                ),
                "removed" => ("Memory removed · This agent", None),
                "loaded" => (
                    "Memory loaded",
                    event
                        .payload
                        .get("injected_context")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                ),
                other => (other, None),
            };
            AgentChatEvent {
                id: format!("memory:{}", event.event_id),
                session_id: event.agent_id,
                provider: "wardian".to_string(),
                kind: AgentChatEventKind::Memory,
                role: Some(AgentChatRole::System),
                text,
                title: Some(title.to_string()),
                status: Some(AgentChatStatus::Succeeded),
                turn_id: None,
                source: Some("wardian_memory".to_string()),
                command: None,
                exit_code: None,
                path: None,
                language: Some("markdown".to_string()),
                created_at: Some(event.occurred_at),
                sequence: None,
                metadata: serde_json::json!({
                    "memory_action": event.action,
                    "memory_id": event.memory_id,
                    "revision_id": event.revision_id,
                    "details": event.payload
                }),
            }
        })
        .collect()
}

fn memory_event_belongs_to_conversation(
    event: &wardian_core::memory::MemoryEvent,
    conversation_started_at: Option<&str>,
) -> bool {
    let Some(started_at) = conversation_started_at else {
        return false;
    };
    match (
        chrono::DateTime::parse_from_rfc3339(&event.occurred_at),
        chrono::DateTime::parse_from_rfc3339(started_at),
    ) {
        (Ok(occurred_at), Ok(started_at)) => occurred_at >= started_at,
        _ => event.occurred_at.as_str() >= started_at,
    }
}

fn sort_chat_events(events: &mut [AgentChatEvent]) {
    events.sort_by(|left, right| {
        match (chat_event_timestamp(left), chat_event_timestamp(right)) {
            (Some(left_timestamp), Some(right_timestamp)) => left_timestamp
                .cmp(&right_timestamp)
                .then_with(|| left.sequence.cmp(&right.sequence)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.sequence.cmp(&right.sequence),
        }
        .then_with(|| left.id.cmp(&right.id))
    });
    for (index, event) in events.iter_mut().enumerate() {
        event.sequence = Some(index as u64 + 1);
    }
}

fn chat_event_timestamp(event: &AgentChatEvent) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    event
        .created_at
        .as_deref()
        .and_then(|created_at| chrono::DateTime::parse_from_rfc3339(created_at).ok())
}

pub(crate) async fn agent_archive_capture_snapshot(
    state: &AppState,
    session_id: &str,
) -> Result<AgentArchiveCaptureSnapshot, String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("session_id is required".to_string());
    }

    let agents = state.agents.lock().await;
    let agent = agents
        .get(&session_id)
        .ok_or_else(|| format!("agent not found: {session_id}"))?;
    let config = agent
        .config
        .lock()
        .map_err(|_| "agent config lock poisoned".to_string())?;
    let provider = config.provider.clone();
    let cleared_provider_sessions = if provider == "codex" {
        config.codex_config().cleared_provider_sessions
    } else {
        Vec::new()
    };
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
    let mut log_path = agent
        .log_path
        .lock()
        .map_err(|_| "agent log path lock poisoned".to_string())?
        .clone();
    let workspace = config
        .git_worktree_folder
        .clone()
        .unwrap_or_else(|| config.folder.clone());

    // Chat may be requested before the provider watcher has performed its
    // first discovery pass, or for an agent restored while off. A persisted
    // provider identity is authoritative for a resumed Antigravity session;
    // do not fall back to the workspace's current mapping because it can
    // belong to the conversation that a fresh launch deliberately replaced.
    if provider == "antigravity" && log_path.is_none() {
        if let Some(path) = config
            .resume_session
            .as_deref()
            .and_then(|conversation_id| {
                AntigravityProvider::antigravity_home().and_then(|home| {
                    AntigravityProvider::conversation_log_path(&home, conversation_id)
                })
            })
        {
            log_path = Some(path.clone());
            if let Ok(mut agent_log_path) = agent.log_path.lock() {
                *agent_log_path = Some(path);
            }
        }
    }
    if provider == "pi" && log_path.is_none() {
        let provider_session_id = config
            .resume_session
            .as_deref()
            .or(config.fresh_provider_session_id.as_deref());
        if let Some(path) = provider_session_id.and_then(|provider_session_id| {
            PiProvider::session_dir(&config.session_id)
                .and_then(|session_dir| PiProvider::session_file(&session_dir, provider_session_id))
        }) {
            log_path = Some(path.clone());
            if let Ok(mut agent_log_path) = agent.log_path.lock() {
                *agent_log_path = Some(path);
            }
        }
    }

    Ok(AgentArchiveCaptureSnapshot {
        session_id,
        provider,
        resume_session: config.resume_session.clone(),
        fresh_provider_session_id: config.fresh_provider_session_id.clone(),
        cleared_provider_sessions,
        current_status,
        last_status_at,
        log_path,
        agent_name: config.session_name.clone(),
        agent_class: config.agent_class.clone(),
        workspace,
        agent_conversation_logging: config.conversation_logging,
        watch_state: agent.watch_state.clone(),
    })
}

pub(crate) fn collect_agent_chat_events_for_archive(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> Result<ArchiveCaptureResult, String> {
    let watch_snapshot = snapshot
        .watch_state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?
        .snapshot_since(None, None)
        .map_err(|error| format!("watch state error: {} {}", error.code(), error.details()))?;
    let mut provider_events = load_provider_log_chat_events(
        &snapshot.session_id,
        &snapshot.provider,
        snapshot.log_path.as_deref(),
        &snapshot.cleared_provider_sessions,
    );
    if snapshot.provider == "opencode" {
        provider_events.extend(load_opencode_db_chat_events(
            &snapshot.session_id,
            opencode_session_id(&snapshot.session_id, snapshot.resume_session.as_deref())
                .as_deref(),
        ));
    }
    let provider_has_transcript = has_transcript_events(&provider_events);
    let watch_events = map_watch_snapshot_to_chat_events(WatchSnapshotChatInput {
        session_id: &snapshot.session_id,
        provider: &snapshot.provider,
        current_status: Some(&snapshot.current_status),
        last_status_at: snapshot.last_status_at.as_deref(),
        events: &watch_snapshot.events,
        output: &watch_snapshot.output,
        transcript: &watch_snapshot.transcript,
        include_transcript: !provider_has_transcript,
        include_terminal_output: !provider_has_transcript,
    });

    let events = merge_chat_events(watch_events, provider_events);
    let context = conversation_archive_context_from_snapshot(snapshot);

    Ok(ArchiveCaptureResult { events, context })
}

pub(crate) async fn archive_agent_chat_events_for_state(
    state: &AppState,
    session_id: &str,
) -> Result<ArchiveCaptureResult, String> {
    let snapshot = agent_archive_capture_snapshot(state, session_id).await?;
    let result = collect_agent_chat_events_for_archive(&snapshot)?;
    let global_conversation_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    if effective_conversation_logging(
        global_conversation_logging,
        snapshot.agent_conversation_logging,
    ) == ConversationLoggingSetting::Enabled
    {
        if let Err(error) = state
            .conversation_archive
            .append_chat_events_with_context(result.context.clone(), &result.events)
        {
            manager::log_debug(&format!(
                "[WARDIAN] conversation archive append failed for {}: {error}",
                snapshot.session_id
            ));
        }
    } else if let Err(error) = state
        .conversation_archive
        .discard_agent_with_context(result.context.clone(), &result.events)
    {
        manager::log_debug(&format!(
            "[WARDIAN] conversation archive disabled cutoff failed for {}: {error}",
            snapshot.session_id
        ));
    }

    Ok(result)
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
    cleared_provider_sessions: &[String],
) -> Vec<AgentChatEvent> {
    let Some(path) = log_path else {
        return Vec::new();
    };
    if provider_log_path_is_cleared(provider, path, cleared_provider_sessions) {
        return Vec::new();
    }
    if provider == "antigravity" && path.extension().is_some_and(|extension| extension == "db") {
        return load_antigravity_database_chat_events(session_id, provider, path);
    }
    let Ok(content) = read_provider_log_tail(path) else {
        return Vec::new();
    };

    let lines = content.lines().collect::<Vec<_>>();
    normalize_chat_lines(session_id, provider, lines.iter())
        .into_iter()
        .map(|mut event| {
            set_metadata(&mut event.metadata, "provider_log", true);
            set_metadata(&mut event.metadata, "log_source", "active_agent_log_path");
            set_metadata(
                &mut event.metadata,
                "log_path",
                path.to_string_lossy().to_string(),
            );
            if provider.eq_ignore_ascii_case("claude") {
                let raw_line = event
                    .sequence
                    .and_then(|sequence| sequence.checked_sub(1))
                    .and_then(|index| lines.get(index as usize))
                    .copied();
                if let Some(raw_line) = raw_line {
                    let legacy_id = claude_legacy_provider_log_event_id(&event, path, raw_line);
                    event.id = stable_provider_log_event_id_from_raw_line(&event, path, raw_line);
                    if event.id != legacy_id {
                        set_metadata(
                            &mut event.metadata,
                            "legacy_event_ids",
                            serde_json::json!([legacy_id]),
                        );
                    }
                }
            } else {
                event.id = stable_provider_log_event_id(&event, path);
            }
            event
        })
        .collect()
}

fn claude_legacy_provider_log_event_id(
    event: &AgentChatEvent,
    path: &Path,
    raw_line: &str,
) -> String {
    let mut legacy_event = event.clone();
    // Before provenance normalization, Claude context and provider-internal
    // messages were persisted with the native `user` role. Recreate that role
    // for the field-derived ID so pre-fix archive refs remain aliases of the
    // raw-line ID emitted now.
    if event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::System)
        && matches!(
            event
                .metadata
                .get("input_origin")
                .and_then(serde_json::Value::as_str),
            Some("context_injection" | "provider_internal")
        )
    {
        legacy_event.role = Some(AgentChatRole::User);
    }
    if legacy_event.role.as_ref() == Some(&AgentChatRole::User) {
        if let (Some(current_text), Ok(parsed)) = (
            event.text.as_deref(),
            serde_json::from_str::<serde_json::Value>(raw_line),
        ) {
            if let Some(legacy_text) = find_legacy_claude_user_text(&parsed, current_text) {
                legacy_event.text = Some(legacy_text);
            }
        }
    }
    stable_provider_log_event_id(&legacy_event, path)
}

fn find_legacy_claude_user_text(value: &serde_json::Value, current_text: &str) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let legacy_text =
                legacy_visible_chat_text_for_provider("claude", &AgentChatRole::User, text)?;
            (visible_chat_text_for_provider("claude", &AgentChatRole::User, text).as_deref()
                == Some(current_text))
            .then_some(legacy_text)
        }
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_legacy_claude_user_text(value, current_text)),
        serde_json::Value::Object(values) => values
            .values()
            .find_map(|value| find_legacy_claude_user_text(value, current_text)),
        _ => None,
    }
}

fn stable_provider_log_event_id_from_raw_line(
    event: &AgentChatEvent,
    path: &Path,
    raw_line: &str,
) -> String {
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
    hash.update(raw_line.trim().as_bytes());
    hash.update(b"\0");
    format!(
        "{}:provider_log:{}",
        event.session_id,
        hex_prefix(hash.finalize().as_slice(), 16)
    )
}

fn load_antigravity_database_chat_events(
    session_id: &str,
    provider: &str,
    path: &Path,
) -> Vec<AgentChatEvent> {
    let Ok(messages) = AntigravityProvider::conversation_messages_from_database(path) else {
        return Vec::new();
    };

    messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            let mut event = AgentChatEvent {
                id: String::new(),
                session_id: session_id.to_string(),
                provider: provider.to_string(),
                kind: AgentChatEventKind::Message,
                role: Some(message.role),
                text: Some(message.text),
                title: None,
                status: None,
                turn_id: Some(message.step_index.to_string()),
                source: Some("conversation_database".to_string()),
                command: None,
                exit_code: None,
                path: None,
                language: None,
                created_at: None,
                sequence: Some(index as u64 + 1),
                metadata: serde_json::json!({
                    "provider_log": true,
                    "log_source": "antigravity_conversation_database",
                    "log_path": path.to_string_lossy(),
                    "step_index": message.step_index,
                }),
            };
            event.id = stable_provider_log_event_id(&event, path);
            event
        })
        .collect()
}

fn stable_provider_log_event_id(event: &AgentChatEvent, path: &Path) -> String {
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
        hex_prefix(hash.finalize().as_slice(), 16)
    )
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    bytes
        .iter()
        .take(len)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn provider_log_path_is_cleared(
    provider: &str,
    path: &Path,
    cleared_provider_sessions: &[String],
) -> bool {
    if provider != "codex" || cleared_provider_sessions.is_empty() {
        return false;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    cleared_provider_sessions.iter().any(|session_id| {
        let session_id = session_id.trim();
        !session_id.is_empty() && file_name.contains(session_id)
    })
}

fn read_provider_log_tail(path: &Path) -> std::io::Result<String> {
    read_provider_log_tail_with_limit(path, PROVIDER_LOG_TAIL_BYTES)
}

fn read_provider_log_tail_with_limit(path: &Path, tail_bytes: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len <= tail_bytes {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        return Ok(content);
    }

    let start = file_len.saturating_sub(tail_bytes);
    let read_start = start.saturating_sub(1);
    file.seek(SeekFrom::Start(read_start))?;
    let mut bytes = Vec::with_capacity((file_len - read_start) as usize);
    file.read_to_end(&mut bytes)?;
    let content = if read_start < start && bytes.first() == Some(&b'\n') {
        String::from_utf8_lossy(&bytes[1..]).to_string()
    } else {
        let content = String::from_utf8_lossy(&bytes);
        content
            .split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    };
    Ok(content)
}

struct ConversationArchiveContextInput<'a> {
    session_id: &'a str,
    provider: &'a str,
    agent_name: &'a str,
    agent_class: &'a str,
    workspace: &'a str,
    resume_session: Option<&'a str>,
    fresh_provider_session_id: Option<&'a str>,
    log_path: Option<&'a Path>,
}

fn conversation_archive_context(
    input: ConversationArchiveContextInput<'_>,
) -> ConversationArchiveContext {
    let provider_session_ids = [input.resume_session, input.fresh_provider_session_id]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let provider_source_key = provider_session_ids
        .first()
        .map(|session| format!("{}:session:{session}", input.provider))
        .or_else(|| {
            input
                .log_path
                .map(|path| format!("{}:source:{}", input.provider, path.to_string_lossy()))
        });

    ConversationArchiveContext {
        agent_id: input.session_id.to_string(),
        agent_name: if input.agent_name.trim().is_empty() {
            input.session_id.to_string()
        } else {
            input.agent_name.to_string()
        },
        agent_class: input.agent_class.to_string(),
        workspace: input.workspace.to_string(),
        provider: input.provider.to_string(),
        provider_session_ids,
        provider_source_key,
    }
}

pub(crate) fn conversation_archive_context_from_snapshot(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> ConversationArchiveContext {
    conversation_archive_context(ConversationArchiveContextInput {
        session_id: &snapshot.session_id,
        provider: &snapshot.provider,
        agent_name: &snapshot.agent_name,
        agent_class: &snapshot.agent_class,
        workspace: &snapshot.workspace,
        resume_session: snapshot.resume_session.as_deref(),
        fresh_provider_session_id: snapshot.fresh_provider_session_id.as_deref(),
        log_path: snapshot.log_path.as_deref(),
    })
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
    let mut request_root_id = None;
    for row in rows {
        let row = row.map_err(|err| err.to_string())?;
        let Some(event) = opencode_db_part_to_chat_event(
            wardian_session_id,
            opencode_session_id,
            events.len() as u64 + 1,
            row,
            request_root_id.as_deref(),
        )?
        else {
            continue;
        };
        if event.role == Some(AgentChatRole::User)
            && event.metadata["input_origin"] != "context_injection"
        {
            request_root_id = event
                .metadata
                .get("request_root_id")
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
                .or_else(|| event.turn_id.clone());
        }
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
    request_root_id: Option<&str>,
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

    let editor_context = part
        .get("metadata")
        .and_then(|value| value.get("kind"))
        .and_then(|value| value.as_str())
        == Some("editor_context");
    let mut metadata = serde_json::json!({
        "provider_log": true,
        "opencode_session_id": opencode_session_id,
        "part_id": row.part_id,
        "raw_type": "text",
        "sequence": sequence,
        "part_time_created": row.part_time_created,
        "message_time_created": row.message_time_created,
    });
    if editor_context {
        metadata["input_origin"] = serde_json::json!("context_injection");
        metadata["input_purpose"] = serde_json::json!("editor_context");
        metadata["context_observation"] = serde_json::json!("provider_native");
        metadata["causal_ref"] = serde_json::json!(format!("provider:message:{}", &row.message_id));
        if let Some(request_root_id) = request_root_id {
            metadata["request_root_id"] = serde_json::json!(request_root_id);
        }
    } else {
        match &role {
            AgentChatRole::User => {
                metadata["input_origin"] = serde_json::json!("human_input");
                metadata["input_purpose"] = serde_json::json!("request");
                metadata["context_observation"] = serde_json::json!("provider_native");
                metadata["request_root_id"] = serde_json::json!(&row.message_id);
            }
            AgentChatRole::System => {
                metadata["input_origin"] = serde_json::json!("provider_internal");
                metadata["input_purpose"] = serde_json::json!("internal");
            }
            AgentChatRole::Assistant | AgentChatRole::Tool => {}
        }
    }

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
        metadata,
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
    let mut archived_event_ids = HashSet::new();
    let mut live_event_ids = HashSet::new();
    let mut provider_message_text_seen = HashSet::new();
    let mut provider_message_indexes_by_text = HashMap::new();
    let mut merged = Vec::with_capacity(watch_events.len() + provider_events.len());

    for mut event in provider_events {
        normalize_chat_event_visible_text(&mut event);
        if is_cross_source_archive_duplicate(&event, &archived_event_ids, &live_event_ids) {
            continue;
        }
        remember_archive_event_id(&event, &mut archived_event_ids, &mut live_event_ids);
        let key = chat_event_dedupe_key(&event);
        if seen.insert(key) {
            if let Some(message_key) = chat_message_text_key(&event) {
                if let Some(existing_index) = provider_message_indexes_by_text.get(&message_key) {
                    if should_collapse_provider_message_duplicate(&merged[*existing_index], &event)
                    {
                        if should_prefer_message_duplicate_candidate(
                            &merged[*existing_index],
                            &event,
                        ) {
                            merged[*existing_index] = event;
                        }
                        continue;
                    }
                }
                provider_message_indexes_by_text.insert(message_key.clone(), merged.len());
                provider_message_text_seen.insert(message_key);
            }
            merged.push(event);
        }
    }

    for mut event in watch_events {
        normalize_chat_event_visible_text(&mut event);
        if is_cross_source_archive_duplicate(&event, &archived_event_ids, &live_event_ids) {
            continue;
        }
        remember_archive_event_id(&event, &mut archived_event_ids, &mut live_event_ids);
        if chat_message_text_key(&event)
            .as_ref()
            .is_some_and(|key| provider_message_text_seen.contains(key))
        {
            continue;
        }

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

fn is_cross_source_archive_duplicate(
    event: &AgentChatEvent,
    archived_event_ids: &HashSet<String>,
    live_event_ids: &HashSet<String>,
) -> bool {
    let event_ids = event_identity_ids(event);
    if event_is_archived(event) {
        event_ids
            .iter()
            .any(|event_id| live_event_ids.contains(*event_id))
    } else {
        event_ids
            .iter()
            .any(|event_id| archived_event_ids.contains(*event_id))
    }
}

fn remember_archive_event_id(
    event: &AgentChatEvent,
    archived_event_ids: &mut HashSet<String>,
    live_event_ids: &mut HashSet<String>,
) {
    let event_ids = event_identity_ids(event);
    if event_is_archived(event) {
        archived_event_ids.extend(event_ids.into_iter().map(ToString::to_string));
    } else {
        live_event_ids.extend(event_ids.into_iter().map(ToString::to_string));
    }
}

fn event_is_archived(event: &AgentChatEvent) -> bool {
    event
        .metadata
        .get("conversation_archive_id")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
}

fn event_identity_ids(event: &AgentChatEvent) -> Vec<&str> {
    let mut ids = vec![event.id.as_str()];
    if let Some(aliases) = event
        .metadata
        .get("legacy_event_ids")
        .and_then(serde_json::Value::as_array)
    {
        ids.extend(aliases.iter().filter_map(serde_json::Value::as_str));
    }
    ids
}

/// Normalizes archived records written before provider adapters learned to
/// remove their internal wrappers. This keeps archive replay on the same
/// visible-text and provenance contract as newly parsed provider events.
fn normalize_chat_event_visible_text(event: &mut AgentChatEvent) {
    normalize_chat_event_provenance(event);
    if event.kind != AgentChatEventKind::Message {
        return;
    }
    let (Some(role), Some(text)) = (event.role.as_ref(), event.text.as_deref()) else {
        return;
    };
    event.text = visible_chat_text_for_provider(&event.provider, role, text);
}

/// Older archives persisted Claude's native `user` role for provider context,
/// interruption markers, and tool-result records. Their explicit provenance
/// metadata is authoritative, so replay can migrate the presentation role
/// without rewriting the durable archive or treating the row as a new prompt.
fn normalize_chat_event_provenance(event: &mut AgentChatEvent) {
    let input_origin = event
        .metadata
        .get("input_origin")
        .and_then(serde_json::Value::as_str);
    if event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && matches!(
            input_origin,
            Some("context_injection" | "provider_internal")
        )
    {
        event.role = Some(AgentChatRole::System);
    } else if event.kind == AgentChatEventKind::ToolResult
        && event.role == Some(AgentChatRole::User)
    {
        event.role = Some(AgentChatRole::Tool);
    }
}

fn should_collapse_provider_message_duplicate(
    existing: &AgentChatEvent,
    candidate: &AgentChatEvent,
) -> bool {
    if existing.kind != AgentChatEventKind::Message || candidate.kind != AgentChatEventKind::Message
    {
        return false;
    }
    if existing.provider != candidate.provider || existing.role != candidate.role {
        return false;
    }
    if normalized_dedupe_text(existing.text.as_deref().unwrap_or(""))
        != normalized_dedupe_text(candidate.text.as_deref().unwrap_or(""))
    {
        return false;
    }
    existing.source != candidate.source
}

fn should_prefer_message_duplicate_candidate(
    existing: &AgentChatEvent,
    candidate: &AgentChatEvent,
) -> bool {
    match (existing.turn_id.as_deref(), candidate.turn_id.as_deref()) {
        (None, Some(_)) => return true,
        (Some(_), None) => return false,
        _ => {}
    }

    source_rank(candidate.source.as_deref()) > source_rank(existing.source.as_deref())
}

fn source_rank(source: Option<&str>) -> u8 {
    match source {
        Some("response_item" | "item.completed") => 2,
        Some("event_msg") => 1,
        _ => 0,
    }
}

fn chat_event_dedupe_key(event: &AgentChatEvent) -> String {
    if let Some(conversation_id) = event
        .metadata
        .get("conversation_archive_id")
        .and_then(|value| value.as_str())
    {
        return format!("archive|{conversation_id}|{}", event.id);
    }

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

fn chat_message_text_key(event: &AgentChatEvent) -> Option<String> {
    if event.kind != AgentChatEventKind::Message {
        return None;
    }
    let text = normalized_dedupe_text(event.text.as_deref().unwrap_or(""));
    if text.is_empty() {
        return None;
    }
    Some(format!(
        "{:?}|{:?}|{}|{}",
        event.kind, event.role, event.provider, text
    ))
}

fn normalized_dedupe_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    let title = provider_launch_title(&provider, &output.text)
        .map(|title| {
            set_metadata(&mut metadata, "terminal_presentation", "launch");
            title.to_string()
        })
        .unwrap_or_else(|| "Terminal output".to_string());

    AgentChatEvent {
        id: event_id(session_id, sequence, "terminal_output"),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::TerminalOutput,
        role: None,
        text: Some(output.text.clone()),
        title: Some(title),
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

/// Provider TUIs commonly write a branded startup screen before their
/// structured transcript becomes available. Preserve that screen as terminal
/// evidence, but mark it for the chat UI to render as a compact lifecycle row.
fn provider_launch_title(provider: &str, output: &str) -> Option<&'static str> {
    let output = output.to_ascii_lowercase();
    match provider {
        "codex" if output.contains("codex") => Some("Codex started"),
        "claude" if output.contains("claude code") || output.contains("claude") => {
            Some("Claude started")
        }
        "gemini" if output.contains("gemini") => Some("Gemini started"),
        "opencode" if output.contains("opencode") => Some("OpenCode started"),
        "antigravity" if output.contains("antigravity") => Some("Antigravity started"),
        "pi" if output.contains("pi coding agent") || output.contains("pi v") => Some("Pi started"),
        _ => None,
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

        let chat_events = load_provider_log_chat_events("agent-1", "codex", Some(&log_path), &[]);

        assert_eq!(chat_events.len(), 2);
        assert_eq!(chat_events[0].kind, AgentChatEventKind::Message);
        assert_eq!(chat_events[0].role, Some(AgentChatRole::User));
        assert_eq!(chat_events[1].role, Some(AgentChatRole::Assistant));
        assert_eq!(
            chat_events[1].text.as_deref(),
            Some("Rendered from the provider log")
        );
        assert_eq!(chat_events[0].metadata["provider_log"], true);
        assert_eq!(
            chat_events[0].metadata["log_path"].as_str(),
            Some(log_path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn skips_codex_provider_log_from_cleared_provider_session() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cleared_session = "019db2f3-22de-7861-8bc6-1b86db1686db";
        let log_path = temp.path().join(format!(
            "rollout-2026-04-20T00-00-00-{cleared_session}.jsonl"
        ));
        std::fs::write(
            &log_path,
            r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":"Before clear"}}"#,
        )
        .expect("write log");

        let chat_events = load_provider_log_chat_events(
            "agent-1",
            "codex",
            Some(&log_path),
            &[cleared_session.to_string()],
        );

        assert!(chat_events.is_empty());
    }

    #[test]
    fn reads_provider_log_tail_from_line_boundary() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        let stale_line = format!(
            "{}{}",
            "x".repeat(512),
            r#"{"type":"response_item","turn_id":"old","payload":{"type":"message","role":"assistant","content":"stale"}}"#,
        );
        let latest_line = r#"{"type":"response_item","turn_id":"turn-2","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Recent provider log message"}]}}"#;
        std::fs::write(&log_path, format!("{stale_line}\n{latest_line}\n")).expect("write log");

        let content = read_provider_log_tail_with_limit(&log_path, 256).expect("tail content");

        assert!(!content.contains("stale"));
        assert!(content.contains("Recent provider log message"));
        assert!(content.starts_with('{'));
    }

    #[test]
    fn reads_provider_log_tail_keeps_line_when_tail_starts_on_boundary() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        let stale_line = "stale provider line";
        let latest_line = r#"{"type":"response_item","turn_id":"turn-2","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Boundary provider log message"}]}}"#;
        std::fs::write(&log_path, format!("{stale_line}\n{latest_line}\n")).expect("write log");

        let tail_bytes = latest_line.len() as u64 + 1;
        let content =
            read_provider_log_tail_with_limit(&log_path, tail_bytes).expect("tail content");

        assert!(!content.contains(stale_line));
        assert!(content.contains("Boundary provider log message"));
        assert!(content.starts_with('{'));
    }

    #[test]
    fn provider_log_event_ids_stay_stable_when_line_position_changes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        let target_line = r#"{"type":"response_item","turn_id":"turn-2","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Stable provider log message"}]}}"#;
        std::fs::write(&log_path, format!("{target_line}\n")).expect("write first log");
        let first = load_provider_log_chat_events("agent-1", "codex", Some(&log_path), &[]);
        let first_id = first
            .iter()
            .find(|event| event.text.as_deref() == Some("Stable provider log message"))
            .expect("target event")
            .id
            .clone();

        let earlier_line = r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Earlier provider log message"}]}}"#;
        std::fs::write(&log_path, format!("{earlier_line}\n{target_line}\n"))
            .expect("write shifted log");
        let second = load_provider_log_chat_events("agent-1", "codex", Some(&log_path), &[]);
        let second_id = second
            .iter()
            .find(|event| event.text.as_deref() == Some("Stable provider log message"))
            .expect("target event")
            .id
            .clone();

        assert_eq!(first_id, second_id);
    }

    #[test]
    fn claude_provider_log_ids_use_raw_lines_and_keep_legacy_aliases() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("claude.jsonl");
        let line = r#"{"type":"user","uuid":"request-1","message":{"role":"user","content":"[Wardian message_id=msg-1 interaction_id=ask-1 generation=7 target=agent-1]\nReview this patch"}}"#;
        std::fs::write(&log_path, format!("{line}\n")).expect("write first log");

        let first = load_provider_log_chat_events("agent-1", "claude", Some(&log_path), &[]);
        let first_event = first.first().expect("Claude user event");
        let first_id = first_event.id.clone();
        let legacy_id = first_event
            .metadata
            .get("legacy_event_ids")
            .and_then(serde_json::Value::as_array)
            .and_then(|ids| ids.first())
            .and_then(serde_json::Value::as_str)
            .expect("legacy event ID alias");

        let mut legacy_event = first_event.clone();
        legacy_event.text = Some(
            "[Wardian message_id=msg-1 interaction_id=ask-1 generation=7 target=agent-1]\nReview this patch"
                .to_string(),
        );
        assert_eq!(
            legacy_id,
            stable_provider_log_event_id(&legacy_event, &log_path)
        );
        assert_ne!(first_id, legacy_id);
        assert_eq!(first_event.text.as_deref(), Some("Review this patch"));

        let earlier_line = r#"{"type":"user","uuid":"request-0","message":{"role":"user","content":"Earlier request"}}"#;
        std::fs::write(&log_path, format!("{earlier_line}\n{line}\n")).expect("write shifted log");
        let second = load_provider_log_chat_events("agent-1", "claude", Some(&log_path), &[]);
        let second_event = second
            .iter()
            .find(|event| event.text.as_deref() == Some("Review this patch"))
            .expect("shifted Claude user event");

        assert_eq!(second_event.id, first_id);
        assert_eq!(
            second_event.metadata["legacy_event_ids"][0].as_str(),
            Some(legacy_id)
        );
    }

    #[test]
    fn claude_provider_log_ids_keep_aliases_for_unchanged_event_kinds() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("claude.jsonl");
        let lines = [
            r#"{"type":"user","uuid":"request-1","message":{"role":"user","content":"An ordinary request"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":"An ordinary answer"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"README.md"}}]}}"#,
            r#"{"type":"result","subtype":"success","result":"completed"}"#,
        ];
        std::fs::write(&log_path, format!("{}\n", lines.join("\n"))).expect("write log");

        let events = load_provider_log_chat_events("agent-1", "claude", Some(&log_path), &[]);

        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| event.metadata["legacy_event_ids"]
            .as_array()
            .is_some_and(|ids| ids.len() == 1 && ids[0].as_str() != Some(event.id.as_str()))));
        assert!(events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::ToolCall));
        assert!(events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::Status));
    }

    #[test]
    fn claude_provider_log_ids_alias_pre_migration_provenance_roles() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("claude.jsonl");
        let lines = [
            r#"{"type":"user","isMeta":true,"uuid":"context-1","message":{"role":"user","content":"Native provider context."}}"#,
            r#"{"type":"user","parentUuid":"assistant-1","message":{"role":"user","content":"[Request interrupted by user]"}}"#,
        ];
        std::fs::write(&log_path, format!("{}\n", lines.join("\n"))).expect("write log");

        let events = load_provider_log_chat_events("agent-1", "claude", Some(&log_path), &[]);

        assert_eq!(events.len(), 2);
        for event in events {
            assert_eq!(event.role, Some(AgentChatRole::System));
            assert!(matches!(
                event.metadata["input_origin"].as_str(),
                Some("context_injection" | "provider_internal")
            ));
            let legacy_id = event
                .metadata
                .get("legacy_event_ids")
                .and_then(serde_json::Value::as_array)
                .and_then(|ids| ids.first())
                .and_then(serde_json::Value::as_str)
                .expect("pre-migration role alias");
            let mut pre_migration_event = event.clone();
            pre_migration_event.role = Some(AgentChatRole::User);
            assert_eq!(
                legacy_id,
                stable_provider_log_event_id(&pre_migration_event, &log_path)
            );
            assert_ne!(legacy_id, event.id);
        }
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
    fn provider_launch_screens_are_marked_for_compact_chat_presentation() {
        for (provider, launch_output, title) in [
            ("codex", "OpenAI Codex\nmodel", "Codex started"),
            ("claude", "Claude Code\nready", "Claude started"),
            ("gemini", "Gemini CLI\nready", "Gemini started"),
            ("opencode", "OpenCode\nready", "OpenCode started"),
            ("antigravity", "Antigravity\nready", "Antigravity started"),
        ] {
            let event = terminal_output_event("agent-1", provider, 1, &output(launch_output));

            assert_eq!(event.title.as_deref(), Some(title));
            assert_eq!(event.metadata["terminal_presentation"], "launch");
        }
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

        let chat_events = merge_chat_events(vec![duplicate], vec![first]);

        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].text.as_deref(), Some("Same answer"));
    }

    #[test]
    fn merge_deduplicates_codex_stream_and_completed_assistant_messages() {
        let first = AgentChatEvent {
            id: "agent-1:provider:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some("Created #daily-task-list under General.".to_string()),
            title: None,
            status: None,
            turn_id: None,
            source: Some("event_msg".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({"provider_log": true}),
        };
        let mut completed = first.clone();
        completed.id = "agent-1:provider:2".to_string();
        completed.text = Some(
        "Created #daily-task-list under General.\n\n<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-1|note=[internal]\n</citation_entries>\n</oai-mem-citation>".to_string(),
    );
        completed.turn_id =
            Some("msg_003d4bf15d017fea016a460ea8668481938d3c49f567fe9108".to_string());
        completed.source = Some("response_item".to_string());

        let chat_events = merge_chat_events(Vec::new(), vec![first, completed]);

        assert_eq!(chat_events.len(), 1);
        assert_eq!(
            chat_events[0].turn_id.as_deref(),
            Some("msg_003d4bf15d017fea016a460ea8668481938d3c49f567fe9108")
        );
        assert_eq!(
            chat_events[0].text.as_deref(),
            Some("Created #daily-task-list under General.")
        );
    }

    #[test]
    fn merge_deduplicates_codex_image_prompt_transport_marker() {
        let provider_events = normalize_chat_lines(
            "agent-1",
            "codex",
            [
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":"<image name=[Image #1] path=\"C:\\Temp\\codex-clipboard.png\"> This is what I see? [Image #1]"}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"This is what I see? [Image #1]"}}"#,
            ],
        );

        let chat_events = merge_chat_events(Vec::new(), provider_events);

        assert_eq!(chat_events.len(), 1);
        assert_eq!(
            chat_events[0].text.as_deref(),
            Some("This is what I see? [Image #1]")
        );
    }

    #[test]
    fn merge_deduplicates_same_message_text_across_distinct_turn_ids() {
        let first = AgentChatEvent {
            id: "agent-1:provider:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some(
                "OK. It seems like you have lots of tiny positions. Are you exiting ever"
                    .to_string(),
            ),
            title: None,
            status: None,
            turn_id: Some("live-input".to_string()),
            source: Some("watch_transcript".to_string()),
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
        duplicate.turn_id = Some("provider-history".to_string());
        duplicate.source = Some("transcript".to_string());

        let chat_events = merge_chat_events(vec![first], vec![duplicate]);

        assert_eq!(chat_events.len(), 1);
        assert_eq!(
            chat_events[0].text.as_deref(),
            Some("OK. It seems like you have lots of tiny positions. Are you exiting ever")
        );
    }

    #[test]
    fn merge_preserves_repeated_same_text_messages_from_the_same_source() {
        let first = AgentChatEvent {
            id: "agent-1:provider:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some("run tests".to_string()),
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
        let mut repeated = first.clone();
        repeated.id = "agent-1:provider:2".to_string();
        repeated.turn_id = Some("turn-2".to_string());

        let chat_events = merge_chat_events(Vec::new(), vec![first, repeated]);

        assert_eq!(chat_events.len(), 2);
    }

    #[test]
    fn merge_preserves_repeated_archived_messages_from_distinct_conversations() {
        let mut first = AgentChatEvent {
            id: "generated:conversation-one:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "antigravity".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some("Repeat this prompt.".to_string()),
            title: None,
            status: None,
            turn_id: None,
            source: Some("wardian_input".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({"conversation_archive_id": "conversation-one"}),
        };
        let mut second = first.clone();
        second.id = "generated:conversation-two:1".to_string();
        second.metadata = serde_json::json!({"conversation_archive_id": "conversation-two"});
        first.sequence = Some(1);
        second.sequence = Some(2);

        let chat_events = merge_chat_events(Vec::new(), vec![first, second]);

        assert_eq!(chat_events.len(), 2);
    }

    #[test]
    fn merge_deduplicates_live_codex_work_events_already_replayed_from_archive() {
        let archived_user = AgentChatEvent {
            id: "agent-1:provider:user".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some("Update the chat timeline.".to_string()),
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
            metadata: serde_json::json!({"conversation_archive_id": "conversation-one"}),
        };
        let mut archived_tool = archived_user.clone();
        archived_tool.id = "agent-1:provider:tool".to_string();
        archived_tool.kind = AgentChatEventKind::ToolCall;
        archived_tool.role = None;
        archived_tool.text = None;
        archived_tool.title = Some("shell_command".to_string());
        archived_tool.command = Some("npm run test".to_string());
        archived_tool.sequence = Some(2);
        let mut archived_assistant = archived_user.clone();
        archived_assistant.id = "agent-1:provider:assistant".to_string();
        archived_assistant.role = Some(AgentChatRole::Assistant);
        archived_assistant.text = Some("The tests passed.".to_string());
        archived_assistant.sequence = Some(3);

        let live_events = [&archived_user, &archived_tool, &archived_assistant]
            .into_iter()
            .cloned()
            .map(|mut event| {
                event.metadata = serde_json::json!({});
                event
            })
            .collect();
        let chat_events = merge_chat_events(
            live_events,
            vec![archived_user, archived_tool, archived_assistant],
        );

        assert_eq!(chat_events.len(), 3);
        assert_eq!(
            chat_events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "agent-1:provider:user",
                "agent-1:provider:tool",
                "agent-1:provider:assistant",
            ]
        );
    }

    #[test]
    fn merge_deduplicates_claude_aliases_for_non_message_events() {
        let archived_tool = AgentChatEvent {
            id: "agent-1:legacy:tool".to_string(),
            session_id: "agent-1".to_string(),
            provider: "claude".to_string(),
            kind: AgentChatEventKind::ToolCall,
            role: None,
            text: None,
            title: Some("shell_command".to_string()),
            status: None,
            turn_id: Some("tool-1".to_string()),
            source: Some("claude_log".to_string()),
            command: Some("npm test".to_string()),
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({"conversation_archive_id": "conversation-one"}),
        };
        let mut archived_result = archived_tool.clone();
        archived_result.id = "agent-1:legacy:result".to_string();
        archived_result.kind = AgentChatEventKind::ToolResult;
        archived_result.text = Some("passed".to_string());
        archived_result.command = None;
        archived_result.source = Some("claude_tool_result".to_string());
        archived_result.sequence = Some(2);
        let mut archived_status = archived_tool.clone();
        archived_status.id = "agent-1:legacy:status".to_string();
        archived_status.kind = AgentChatEventKind::Status;
        archived_status.title = Some("processing".to_string());
        archived_status.status = Some(AgentChatStatus::Processing);
        archived_status.text = Some("Working".to_string());
        archived_status.turn_id = None;
        archived_status.command = None;
        archived_status.source = Some("claude_status".to_string());
        archived_status.sequence = Some(3);

        let archived_events = vec![archived_tool, archived_result, archived_status];
        let live_events = archived_events
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, mut event)| {
                let legacy_id = event.id.clone();
                event.id = format!("agent-1:raw:{index}");
                event.metadata = serde_json::json!({"legacy_event_ids": [legacy_id]});
                event
            })
            .collect();

        let chat_events = merge_chat_events(live_events, archived_events);

        assert_eq!(chat_events.len(), 3);
        assert!(chat_events
            .iter()
            .all(|event| event.metadata.get("conversation_archive_id").is_some()));
        assert!(chat_events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::ToolCall));
        assert!(chat_events
            .iter()
            .any(|event| event.kind == AgentChatEventKind::Status));
    }

    #[test]
    fn merge_replays_legacy_claude_provenance_roles_without_false_prompts() {
        let archived_context = AgentChatEvent {
            id: "agent-1:legacy:context".to_string(),
            session_id: "agent-1".to_string(),
            provider: "claude".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some("Native provider context.".to_string()),
            title: None,
            status: None,
            turn_id: Some("context-1".to_string()),
            source: Some("stream_json".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(1),
            metadata: serde_json::json!({
                "conversation_archive_id": "conversation-one",
                "input_origin": "context_injection",
                "input_purpose": "context"
            }),
        };
        let mut archived_internal = archived_context.clone();
        archived_internal.id = "agent-1:legacy:internal".to_string();
        archived_internal.text = Some("[Request interrupted by user]".to_string());
        archived_internal.turn_id = None;
        archived_internal.sequence = Some(2);
        archived_internal.metadata["input_origin"] = serde_json::json!("provider_internal");
        archived_internal.metadata["input_purpose"] = serde_json::json!("internal");
        let mut archived_result = archived_context.clone();
        archived_result.id = "agent-1:legacy:result".to_string();
        archived_result.kind = AgentChatEventKind::ToolResult;
        archived_result.role = Some(AgentChatRole::User);
        archived_result.text = Some("1\tWARDIAN_CLAUDE_REAL_EVIDENCE".to_string());
        archived_result.turn_id = Some("toolu_01evidence".to_string());
        archived_result.sequence = Some(3);
        archived_result.metadata = serde_json::json!({
            "conversation_archive_id": "conversation-one",
            "raw_type": "tool_result"
        });

        let mut live_context = archived_context.clone();
        live_context.id = "agent-1:raw:context".to_string();
        live_context.role = Some(AgentChatRole::System);
        live_context.metadata = serde_json::json!({
            "input_origin": "context_injection",
            "legacy_event_ids": ["agent-1:legacy:context"]
        });
        let mut live_internal = archived_internal.clone();
        live_internal.id = "agent-1:raw:internal".to_string();
        live_internal.role = Some(AgentChatRole::System);
        live_internal.metadata = serde_json::json!({
            "input_origin": "provider_internal",
            "legacy_event_ids": ["agent-1:legacy:internal"]
        });

        let replayed = merge_chat_events(
            vec![archived_context, archived_internal, archived_result],
            vec![live_context, live_internal],
        );

        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].role, Some(AgentChatRole::System));
        assert_eq!(replayed[0].metadata["input_origin"], "context_injection");
        assert_eq!(replayed[1].role, Some(AgentChatRole::System));
        assert_eq!(replayed[1].metadata["input_origin"], "provider_internal");
        assert_eq!(replayed[2].kind, AgentChatEventKind::ToolResult);
        assert_eq!(replayed[2].role, Some(AgentChatRole::Tool));
    }

    #[test]
    fn antigravity_sqlite_conversation_renders_user_and_assistant_history() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let connection = rusqlite::Connection::open(&database).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);",
            )
            .expect("create steps");
        // field 19.2 is the current Antigravity user message; field 20.1 is
        // the completed planner response in its SQLite step payload.
        let user = vec![0x9a, 0x01, 0x05, 0x12, 0x03, b'h', b'i', b'!'];
        let assistant = vec![0xa2, 0x01, 0x05, 0x0a, 0x03, b'o', b'k', b'!'];
        connection
            .execute(
                "INSERT INTO steps VALUES (0, 14, ?1)",
                rusqlite::params![user],
            )
            .expect("insert user");
        connection
            .execute(
                "INSERT INTO steps VALUES (1, 15, ?1)",
                rusqlite::params![assistant],
            )
            .expect("insert assistant");

        let events = load_antigravity_database_chat_events("agent-1", "antigravity", &database);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].role, Some(AgentChatRole::User));
        assert_eq!(events[0].text.as_deref(), Some("hi!"));
        assert_eq!(events[1].role, Some(AgentChatRole::Assistant));
        assert_eq!(events[1].text.as_deref(), Some("ok!"));
        assert_eq!(
            events[1].metadata["log_source"],
            "antigravity_conversation_database"
        );
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
            INSERT INTO message VALUES ('msg-context', 'ses_test', 3, 3, '{"role":"user"}');
            INSERT INTO part VALUES ('part-context', 'msg-context', 'ses_test', 4, 4, '{"type":"text","text":"<system-reminder>Editor context</system-reminder>","synthetic":true,"metadata":{"kind":"editor_context","source":"websocket"}}');
            INSERT INTO message VALUES ('msg-assistant', 'ses_test', 3, 3, '{"role":"assistant"}');
            INSERT INTO part VALUES ('part-finish', 'msg-assistant', 'ses_test', 5, 5, '{"type":"finish","reason":"stop"}');
            INSERT INTO part VALUES ('part-assistant', 'msg-assistant', 'ses_test', 6, 6, '{"type":"text","text":"1, 2, 3"}');
            "#,
        )
        .expect("seed db");

        let chat_events =
            load_opencode_db_chat_events_from_db(&db_path, "agent-1", "ses_test").expect("load db");

        assert_eq!(chat_events.len(), 3);
        assert_eq!(chat_events[0].provider, "opencode");
        assert_eq!(chat_events[0].role, Some(AgentChatRole::User));
        assert_eq!(chat_events[0].text.as_deref(), Some("List 50 numbers."));
        assert_eq!(chat_events[0].metadata["input_origin"], "human_input");
        assert_eq!(chat_events[1].role, Some(AgentChatRole::User));
        assert_eq!(chat_events[1].metadata["input_origin"], "context_injection");
        assert_eq!(chat_events[1].metadata["input_purpose"], "editor_context");
        assert_eq!(
            chat_events[1].metadata["context_observation"],
            "provider_native"
        );
        assert_eq!(chat_events[1].metadata["request_root_id"], "msg-user");
        assert_eq!(
            chat_events[1].metadata["causal_ref"],
            "provider:message:msg-context"
        );
        assert_eq!(chat_events[2].role, Some(AgentChatRole::Assistant));
        assert_eq!(chat_events[2].text.as_deref(), Some("1, 2, 3"));
        assert_eq!(chat_events[2].source.as_deref(), Some("opencode_db"));
        assert_eq!(chat_events[2].metadata["opencode_session_id"], "ses_test");
        assert_eq!(chat_events[2].metadata["part_id"], "part-assistant");
        assert_eq!(chat_events[2].metadata["raw_type"], "text");
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

    #[test]
    fn memory_events_before_active_conversation_are_not_replayed() {
        let before = wardian_core::memory::MemoryEvent {
            event_id: "before".into(),
            agent_id: "agent-1".into(),
            memory_id: None,
            revision_id: None,
            action: "loaded".into(),
            payload: serde_json::Value::Null,
            occurred_at: "2026-08-24T10:00:00.000Z".into(),
        };
        let after = wardian_core::memory::MemoryEvent {
            occurred_at: "2026-08-24T11:00:00.000Z".into(),
            event_id: "after".into(),
            ..before.clone()
        };

        assert!(!memory_event_belongs_to_conversation(
            &before,
            Some("2026-08-24T10:30:00.000Z")
        ));
        assert!(!memory_event_belongs_to_conversation(&after, None));
        assert!(memory_event_belongs_to_conversation(
            &after,
            Some("2026-08-24T10:30:00.000Z")
        ));
    }

    #[test]
    fn memory_events_are_interleaved_with_chat_events_by_timestamp() {
        let mut events = vec![
            AgentChatEvent {
                id: "chat-late".into(),
                session_id: "agent-1".into(),
                provider: "mock".into(),
                kind: AgentChatEventKind::Message,
                role: Some(AgentChatRole::Assistant),
                text: Some("late".into()),
                title: None,
                status: None,
                turn_id: None,
                source: None,
                command: None,
                exit_code: None,
                path: None,
                language: None,
                created_at: Some("2026-08-24T11:00:00.000Z".into()),
                sequence: Some(1),
                metadata: serde_json::Value::Null,
            },
            AgentChatEvent {
                id: "memory-middle".into(),
                session_id: "agent-1".into(),
                provider: "wardian".into(),
                kind: AgentChatEventKind::Memory,
                role: Some(AgentChatRole::System),
                text: None,
                title: Some("Memory loaded".into()),
                status: Some(AgentChatStatus::Succeeded),
                turn_id: None,
                source: Some("wardian_memory".into()),
                command: None,
                exit_code: None,
                path: None,
                language: Some("markdown".into()),
                created_at: Some("2026-08-24T10:30:00.000Z".into()),
                sequence: Some(2),
                metadata: serde_json::Value::Null,
            },
            AgentChatEvent {
                id: "memory-offset".into(),
                session_id: "agent-1".into(),
                provider: "wardian".into(),
                kind: AgentChatEventKind::Memory,
                role: Some(AgentChatRole::System),
                text: None,
                title: Some("Memory saved".into()),
                status: Some(AgentChatStatus::Succeeded),
                turn_id: None,
                source: Some("wardian_memory".into()),
                command: None,
                exit_code: None,
                path: None,
                language: Some("markdown".into()),
                created_at: Some("2026-08-24T08:30:00.000+02:00".into()),
                sequence: Some(3),
                metadata: serde_json::Value::Null,
            },
            AgentChatEvent {
                id: "chat-unknown".into(),
                session_id: "agent-1".into(),
                provider: "mock".into(),
                kind: AgentChatEventKind::Status,
                role: Some(AgentChatRole::System),
                text: Some("Idle".into()),
                title: None,
                status: Some(AgentChatStatus::Succeeded),
                turn_id: None,
                source: None,
                command: None,
                exit_code: None,
                path: None,
                language: None,
                created_at: None,
                sequence: Some(4),
                metadata: serde_json::Value::Null,
            },
        ];

        sort_chat_events(&mut events);

        assert_eq!(events[0].id, "memory-offset");
        assert_eq!(events[1].id, "memory-middle");
        assert_eq!(events[2].id, "chat-late");
        assert_eq!(events[3].id, "chat-unknown");
        assert_eq!(events[0].sequence, Some(1));
        assert_eq!(events[1].sequence, Some(2));
        assert_eq!(events[3].sequence, Some(4));
    }
}
