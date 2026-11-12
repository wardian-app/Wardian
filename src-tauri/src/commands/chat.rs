use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::manager::{
    self,
    opencode::{opencode_database_path, opencode_log_dirs, opencode_log_path_in},
};
use crate::providers::antigravity::AntigravityProvider;
use crate::providers::chat_transcript::{
    codex_user_mirror_pair, legacy_visible_chat_text_for_provider, normalize_chat_lines,
    visible_chat_text, visible_chat_text_for_provider, PROVIDER_LOG_ROW_OFFSET_METADATA_KEY,
    PROVIDER_RAW_LINE_METADATA_KEY,
};
use crate::providers::pi::PiProvider;
use crate::state::background_capture::{CaptureRequest, CaptureStop};
use crate::state::conversation_archive::{
    effective_conversation_logging, ConversationArchiveContext,
};
use crate::state::{AgentWatchState, AppState};
use sha2::{Digest, Sha256};
use tauri::{Manager, State};
use wardian_core::control::{WatchEvent, WatchOutput, WatchTranscript, WatchTranscriptMessage};
use wardian_core::conversations::{AgentConversationLoggingSetting, ConversationLoggingSetting};
use wardian_core::identity::normalize_status;
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

#[path = "chat_archive_identity.rs"]
pub(crate) mod archive_identity;
use archive_identity::{
    attach_provider_log_row_offsets, can_alias_legacy_provider_log_event_id,
    legacy_provider_log_event_id, provider_log_row_offsets, requires_provider_log_row_identity,
    stable_provider_log_event_id, with_provider_log_row_identity,
};

#[path = "chat_restore_source.rs"]
mod restore_source;

const PROVIDER_LOG_TAIL_BYTES: u64 = 2 * 1024 * 1024;
const SLOW_LIFECYCLE_POLICY_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Normal display reads are independent of provider capture and archive repair.
/// Optional source revalidation is acknowledged only against the current complete index.
#[tauri::command]
pub async fn load_agent_chat_page(
    session_id: String,
    cursor: Option<String>,
    revision: Option<String>,
    detail_ref: Option<String>,
    revalidate_ids: Option<Vec<String>>,
    recent_preview: Option<bool>,
    app: tauri::AppHandle,
) -> Result<wardian_core::models::chat::AgentChatPage, String> {
    let state = app.state::<AppState>();
    let queue_error = if recent_preview == Some(true)
        && cursor.is_none()
        && detail_ref.is_none()
        && revalidate_ids.is_none()
        && !session_id.trim().is_empty()
        && revision.as_ref().is_none_or(|value| value.len() <= 512)
    {
        queue_recent_capture_for_app(&state, &app, &session_id).err()
    } else {
        None
    };
    let page = load_agent_chat_page_with_recent_for_state(
        &state,
        session_id,
        cursor,
        revision,
        detail_ref,
        revalidate_ids,
        RecentChatRead {
            limit: None,
            preview: recent_preview.unwrap_or(false),
        },
    )
    .await?;
    // Retirement resets and already readable pages remain useful even when
    // the two producer slots are busy. A cold empty miss is retryable.
    if page.events.is_empty() && !page.reset && queue_error.is_some() {
        return Err("chat snapshot temporarily busy".into());
    }
    Ok(page)
}

/// Coalesce a bounded selection prelude without doing source IO on the request
/// thread. Capacity stays owned until the actual blocking job has joined.
pub(crate) fn queue_recent_capture_for_app<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    session_id: &str,
) -> Result<(), String> {
    let (snapshot, runtime) = chat_read_snapshot_for_state(state, session_id)?;
    let nonce = state
        .conversation_archive
        .chat_projection
        .request_recent(&snapshot, &runtime)
        .map_err(|_| "chat snapshot temporarily busy".to_string())?;
    if let Some(nonce) = nonce {
        let owner_app = app.clone();
        let producer_app = app.clone();
        let session_id = session_id.to_owned();
        tauri::async_runtime::spawn(async move {
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let state = producer_app.state::<AppState>();
                publish_recent_for_state(&state, &session_id, Some(&runtime))
            })
            .await;
            owner_app
                .state::<AppState>()
                .conversation_archive
                .chat_projection
                .finish_recent_request(&nonce);
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
pub(crate) struct RecentChatRead {
    pub(crate) limit: Option<usize>,
    pub(crate) preview: bool,
}

pub(crate) async fn load_agent_chat_page_with_recent_for_state(
    state: &AppState,
    session_id: String,
    cursor: Option<String>,
    revision: Option<String>,
    detail_ref: Option<String>,
    revalidate_ids: Option<Vec<String>>,
    recent: RecentChatRead,
) -> Result<wardian_core::models::chat::AgentChatPage, String> {
    let recent_limit = recent.limit;
    let revalidation_requested = revalidate_ids.is_some();
    if session_id.trim().is_empty()
        || (recent.preview && (cursor.is_some() || detail_ref.is_some() || revalidation_requested))
        || recent_limit.is_some_and(|limit| {
            !(1..=crate::state::conversation_archive::chat_read::PAGE_ROWS).contains(&limit)
        })
        || cursor.as_ref().is_some_and(|value| value.len() > 512)
        || revision.as_ref().is_some_and(|value| value.len() > 512)
        || detail_ref.as_ref().is_some_and(|value| value.len() > 512)
    {
        return Err(if revalidation_requested {
            "chat projection read unavailable"
        } else {
            "invalid chat read request"
        }
        .into());
    }
    let revalidate_ids =
        crate::state::conversation_archive::chat_read::validate_revalidation_request(
            &session_id,
            cursor.as_deref(),
            revision.as_deref(),
            detail_ref.as_deref(),
            revalidate_ids,
        )
        .map_err(|_| {
            if revalidation_requested {
                "chat projection read unavailable".to_string()
            } else {
                "invalid chat read request".to_string()
            }
        })?;
    // The normal read uses cached runtime identity only. It never discovers
    // provider paths or queues behind a lifecycle write to the roster.
    let (snapshot, incarnation) = chat_read_snapshot_for_state(state, &session_id)?;
    let context = conversation_archive_context_from_snapshot(&snapshot);
    let expected_source = context.provider_source_key.clone();
    let previous_preview = recent.preview
        && state
            .conversation_archive
            .chat_projection
            .owns_recent_revision(revision.as_deref());
    let recent_reader = recent.preview.then(|| {
        state.conversation_archive.chat_projection.recent_reader(
            snapshot.clone(),
            incarnation.clone(),
            revision.clone(),
            recent_limit.unwrap_or(80),
        )
    });
    let result = tauri::async_runtime::spawn_blocking(move || {
        if let Some(read) = recent_reader {
            if let Some(page) = read()? {
                return Ok(page);
            }
        }
        crate::state::conversation_archive::chat_read::read_with_revalidation_and_limit(
            &context,
            &snapshot,
            cursor.as_deref(),
            revision.as_deref(),
            detail_ref.as_deref(),
            revalidate_ids.as_deref(),
            recent_limit,
        )
    })
    .await
    .map_err(|_| "chat read task failed".to_string())?
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidInput {
            "invalid chat read request".to_string()
        } else {
            "chat projection read unavailable".to_string()
        }
    })?;
    let current = chat_read_snapshot_for_state(state, &session_id)?;
    if !Arc::ptr_eq(&incarnation, &current.1)
        || conversation_archive_context_from_snapshot(&current.0).provider_source_key
            != expected_source
    {
        return Err("chat runtime changed during read".into());
    }
    let mut result = result;
    if previous_preview && result.recent_admission.is_none() {
        result.reset = true;
        result.unchanged = false;
    }
    if result.revalidated_ids.is_some()
        && !crate::state::conversation_archive::chat_read::revalidation_page_is_current(
            &current.0, &result,
        )
    {
        result.events.clear();
        result.aliases.clear();
        result.removed_ids.clear();
        result.revalidated_ids = None;
        result.generation = None;
        result.source_epoch = None;
        result.reset = true;
        result.progress = "source_changed".into();
    }
    Ok(result)
}

pub(crate) fn chat_read_snapshot_for_state(
    state: &AppState,
    session_id: &str,
) -> Result<(AgentArchiveCaptureSnapshot, Arc<Mutex<String>>), String> {
    let session_id = session_id.to_string();
    let snapshot = {
        let agents = state
            .agents
            .try_lock()
            .map_err(|_| "chat snapshot temporarily busy")?;
        let agent = agents.get(&session_id).ok_or("chat agent unavailable")?;
        let config = agent
            .config
            .try_lock()
            .map_err(|_| "chat config temporarily busy")?;
        let current_status = agent
            .current_status
            .try_lock()
            .map_err(|_| "chat status temporarily busy")?
            .clone();
        let log_path = agent
            .log_path
            .try_lock()
            .map_err(|_| "chat source temporarily busy")?
            .clone();
        (
            AgentArchiveCaptureSnapshot {
                session_id: session_id.clone(),
                provider: config.provider.clone(),
                resume_session: config.resume_session.clone(),
                fresh_provider_session_id: config.fresh_provider_session_id.clone(),
                cleared_provider_sessions: if config.provider == "codex" {
                    config.codex_config().cleared_provider_sessions
                } else {
                    Vec::new()
                },
                current_status,
                last_status_at: None,
                log_path,
                agent_name: String::new(),
                agent_class: String::new(),
                workspace: String::new(),
                agent_conversation_logging: config.conversation_logging,
                watch_state: agent.watch_state.clone(),
            },
            agent.current_status.clone(),
        )
    };
    Ok(snapshot)
}

#[cfg(test)]
#[path = "chat_antigravity_tests.rs"]
mod antigravity_tests;
#[path = "chat_opencode_tools.rs"]
mod opencode_tools;

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
    pub(crate) continue_immediately: bool,
    pub(crate) background_stop: CaptureStop,
}

fn provider_log_source_is_fresh(
    provider: &str,
    resume_session: Option<&str>,
    fresh_provider_session_id: Option<&str>,
) -> bool {
    if !matches!(provider, "claude" | "codex" | "pi" | "mock") {
        return false;
    }
    let Some(fresh) = fresh_provider_session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    match resume_session {
        None => true,
        Some(resume) => {
            let resume = resume.trim();
            !resume.is_empty() && resume == fresh
        }
    }
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
    load_agent_chat_transcript_inner(state, session_id)
        .await
        .map_err(|failure| failure.message)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChatTranscriptFailureStage {
    AgentSnapshot,
    ProviderLogCapture,
    ArchiveWrite,
    ProviderProjection,
    Provenance,
}

struct AgentChatTranscriptFailure {
    stage: ChatTranscriptFailureStage,
    message: String,
}

async fn load_agent_chat_transcript_inner(
    state: &AppState,
    session_id: String,
) -> Result<Vec<AgentChatEvent>, AgentChatTranscriptFailure> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err(AgentChatTranscriptFailure {
            stage: ChatTranscriptFailureStage::AgentSnapshot,
            message: "session_id is required".to_string(),
        });
    }

    let result =
        archive_agent_chat_events_for_state_with_stage(state, &session_id, CaptureLane::Background)
            .await?;
    let mut current_events = result.events;
    let mut archived_events = state
        .conversation_archive
        .chat_events_for_capture(&result.context)
        .unwrap_or_else(|_error| {
            manager::log_debug("[WARDIAN] conversation archive chat replay failed");
            Vec::new()
        });
    coalesce_codex_watch_observations_across(&mut current_events, &mut archived_events);

    // Provider logs and the watch snapshot are live, bounded sources. Replay
    // only the active durable archive so a restart or log rotation does not
    // erase current chat rows, while a new provider session starts empty.
    let mut events = crate::state::conversation_archive::provenance::merge_current_capture(
        current_events,
        archived_events,
    )
    .map_err(|error| AgentChatTranscriptFailure {
        stage: ChatTranscriptFailureStage::Provenance,
        message: format!("conversation archive provenance refresh failed: {error}"),
    })?;
    canonicalize_provider_input_projection(&mut events);
    for event in &mut events {
        normalize_chat_event_visible_text(event);
    }
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
    agent_archive_capture_snapshot_bound(state, session_id, None).await
}

async fn agent_archive_capture_snapshot_bound(
    state: &AppState,
    session_id: &str,
    incarnation: Option<&Arc<Mutex<String>>>,
) -> Result<AgentArchiveCaptureSnapshot, String> {
    restore_source::bind_for_background(state, session_id, incarnation).await?;
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("session_id is required".to_string());
    }

    let agents = state.agents.lock().await;
    let agent = agents
        .get(&session_id)
        .ok_or_else(|| format!("agent not found: {session_id}"))?;
    if incarnation.is_some_and(|expected| !Arc::ptr_eq(&agent.current_status, expected)) {
        return Err("background capture incarnation retired".to_string());
    }
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
    if provider == "opencode" && log_path.is_none() {
        let provider_session_id = opencode_session_id(
            &config.session_id,
            config.resume_session.as_deref(),
            config.fresh_provider_session_id.as_deref(),
        );
        if let Some(path) = provider_session_id.and_then(|provider_session_id| {
            opencode_log_dirs()
                .into_iter()
                .find_map(|directory| opencode_log_path_in(&directory, &provider_session_id))
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
    collect_agent_chat_events_for_archive_bound(snapshot, None)
}

fn collect_agent_chat_events_for_archive_bound(
    snapshot: &AgentArchiveCaptureSnapshot,
    native: Option<&[AgentChatEvent]>,
) -> Result<ArchiveCaptureResult, String> {
    let mut provider_events = load_provider_log_chat_events(
        &snapshot.session_id,
        &snapshot.provider,
        snapshot.log_path.as_deref(),
        &snapshot.cleared_provider_sessions,
    );
    provider_events.extend(
        native
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| load_opencode_db_chat_events_for_snapshot(snapshot)),
    );
    collect_agent_chat_events_with_provider_events(snapshot, provider_events, false)
}

pub(crate) fn collect_agent_chat_events_for_boundary(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> Result<ArchiveCaptureResult, String> {
    let mut provider_events = if snapshot.provider == "antigravity"
        && snapshot
            .log_path
            .as_deref()
            .is_some_and(|path| path.extension().is_some_and(|extension| extension == "db"))
    {
        load_provider_log_chat_events(
            &snapshot.session_id,
            &snapshot.provider,
            snapshot.log_path.as_deref(),
            &snapshot.cleared_provider_sessions,
        )
    } else {
        Vec::new()
    };
    provider_events.extend(load_opencode_db_chat_events_for_snapshot(snapshot));
    collect_agent_chat_events_with_provider_events(snapshot, provider_events, false)
}

fn collect_agent_chat_events_with_provider_events(
    snapshot: &AgentArchiveCaptureSnapshot,
    provider_events: Vec<AgentChatEvent>,
    continue_immediately: bool,
) -> Result<ArchiveCaptureResult, String> {
    let watch_snapshot = snapshot
        .watch_state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?
        .snapshot_since(None, None)
        .map_err(|error| format!("watch state error: {} {}", error.code(), error.details()))?;
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

    Ok(ArchiveCaptureResult {
        events,
        context,
        continue_immediately,
        background_stop: CaptureStop::More,
    })
}

/// Which side of the capture policy gate a pass runs on. See
/// [`crate::state::capture_policy_gate`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureLane {
    /// Best-effort syncs (status, restore, Chat reads) stand aside for a boundary.
    Background,
    /// A lifecycle boundary that must not queue behind background syncs.
    Lifecycle,
}

async fn archive_agent_chat_events_for_state_with_stage(
    state: &AppState,
    session_id: &str,
    lane: CaptureLane,
) -> Result<ArchiveCaptureResult, AgentChatTranscriptFailure> {
    archive_agent_chat_events_bound(state, session_id, lane, None).await
}

async fn archive_agent_chat_events_bound(
    state: &AppState,
    session_id: &str,
    lane: CaptureLane,
    expected: Option<&CaptureRequest>,
) -> Result<ArchiveCaptureResult, AgentChatTranscriptFailure> {
    let _ = publish_recent_for_state(
        state,
        session_id,
        expected.map(|request| &request.incarnation),
    );
    // Lock order is policy gate, then the roster snapshot, then the archive's
    // per-agent gate. The snapshot must follow the gate: a pass that waits for
    // the gate through a lifecycle boundary would otherwise replay the old
    // runtime's identity and logging setting after the archive rolled over.
    // No caller may hold `state.agents` here.
    let policy_wait = std::time::Instant::now();
    let _policy_guard = match lane {
        CaptureLane::Background => {
            state
                .conversation_capture_policy_lock
                .lock_background()
                .await
        }
        CaptureLane::Lifecycle => state.conversation_capture_policy_lock.lock().await,
    };
    if lane == CaptureLane::Lifecycle && policy_wait.elapsed() >= SLOW_LIFECYCLE_POLICY_WAIT {
        crate::manager::log_debug(&format!(
            "[WARDIAN] Lifecycle archive drain for {session_id} waited {} ms for the capture policy gate",
            policy_wait.elapsed().as_millis()
        ));
    }
    let snapshot = agent_archive_capture_snapshot_bound(
        state,
        session_id,
        expected.map(|request| &request.incarnation),
    )
    .await
    .map_err(|message| AgentChatTranscriptFailure {
        stage: ChatTranscriptFailureStage::AgentSnapshot,
        message,
    })?;
    if let Some(expected) = expected {
        if snapshot.provider != expected.provider
            || snapshot
                .resume_session
                .as_ref()
                .or(snapshot.fresh_provider_session_id.as_ref())
                != expected.conversation.as_ref()
            || snapshot.log_path != expected.source_path
        {
            return Err(AgentChatTranscriptFailure {
                stage: ChatTranscriptFailureStage::AgentSnapshot,
                message: "background capture provider/conversation/source retired".to_string(),
            });
        }
    }
    let global_conversation_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    let logging_enabled = effective_conversation_logging(
        global_conversation_logging,
        snapshot.agent_conversation_logging,
    ) == ConversationLoggingSetting::Enabled;
    let context = conversation_archive_context_from_snapshot(&snapshot);

    // This bounded, nonauthoritative prelude precedes saved bootstrap, complete
    // DB merge and legacy archive reads. Failure never skips the durable path.
    let recent_tags = publish_watch_recent(state, &snapshot).unwrap_or_default();

    // Historical display admission has its own bounded checkpoints. It runs
    // under the same identity/policy gates, even at EOF or without a vendor file.
    if expected.is_some() && logging_enabled {
        let pending = state
            .conversation_archive
            .bootstrap_saved_chat(&context)
            .map_err(|error| AgentChatTranscriptFailure {
                stage: ChatTranscriptFailureStage::ArchiveWrite,
                message: format!("saved chat bootstrap failed: {error}"),
            })?;
        if pending {
            return Ok(ArchiveCaptureResult {
                events: Vec::new(),
                context,
                continue_immediately: true,
                background_stop: CaptureStop::More,
            });
        }
    }
    if expected.is_some_and(|request| request.source_path.is_some() && request.source.is_none()) {
        return Err(AgentChatTranscriptFailure {
            stage: ChatTranscriptFailureStage::ProviderLogCapture,
            message: "background provider source observation unavailable".to_string(),
        });
    }

    if let (Some(path), Some(provider_source_key)) = (
        append_only_provider_log_path(&snapshot),
        context.provider_source_key.as_deref(),
    ) {
        let trust_source_from_start = provider_log_source_is_fresh(
            &snapshot.provider,
            snapshot.resume_session.as_deref(),
            snapshot.fresh_provider_session_id.as_deref(),
        );
        let previous = state
            .conversation_archive
            .provider_log_capture_state(&snapshot.session_id, provider_source_key)
            .map_err(|error| AgentChatTranscriptFailure {
                stage: ChatTranscriptFailureStage::ProviderLogCapture,
                message: format!("provider-log capture state read failed: {error}"),
            })?;
        let policy = super::provider_log_acquisition::observe_provider_log_policy_with_identity(
            path,
            provider_source_key,
            previous.clone(),
            logging_enabled,
            trust_source_from_start,
            expected
                .and_then(|request| request.source.as_ref())
                .map(|source| &source.native_identity),
        )
        .map_err(|error| AgentChatTranscriptFailure {
            stage: ChatTranscriptFailureStage::ProviderLogCapture,
            message: format!("provider-log policy observation failed: {error}"),
        })?;
        if let Some(policy) = policy {
            {
                state
                    .conversation_archive
                    .append_provider_log_batch_with_context(
                        context.clone(),
                        &[],
                        previous.as_ref(),
                        &policy.next,
                    )
                    .map_err(|error| AgentChatTranscriptFailure {
                        stage: ChatTranscriptFailureStage::ArchiveWrite,
                        message: format!("provider-log policy commit failed: {error}"),
                    })?;
            }
            if expected.is_some() && !logging_enabled {
                // The background owner observes the existing disabled span
                // and watch cutoff, then suspends. Enabled backlog stays at
                // its cursor until a later policy observation re-enables it.
                let mut result =
                    collect_agent_chat_events_with_provider_events(&snapshot, Vec::new(), false)
                        .map_err(|message| AgentChatTranscriptFailure {
                            stage: ChatTranscriptFailureStage::ProviderProjection,
                            message,
                        })?;
                state
                    .conversation_archive
                    .discard_agent_with_context(context, &result.events)
                    .map_err(|error| AgentChatTranscriptFailure {
                        stage: ChatTranscriptFailureStage::ArchiveWrite,
                        message: format!("conversation archive disabled cutoff failed: {error}"),
                    })?;
                result.background_stop = CaptureStop::Disabled;
                return Ok(result);
            }
            // Cold EOF can have no canonical candidate. Publish independently
            // admitted recent observations before any legacy archive replay.
            let source_index_pending = if expected.is_some() && logging_enabled {
                crate::state::conversation_archive::chat_read::advance_source_index(&snapshot)
                    .map_err(|_| AgentChatTranscriptFailure {
                        stage: ChatTranscriptFailureStage::ProviderProjection,
                        message: "source projection checkpoint failed".into(),
                    })?
            } else {
                false
            };
            let batch = if let Some(expected) = expected {
                super::provider_log_acquisition::acquire_provider_log_batch_for_identity(
                    &snapshot.session_id,
                    &snapshot.provider,
                    path,
                    provider_source_key,
                    Some(policy.next),
                    trust_source_from_start,
                    expected
                        .source
                        .as_ref()
                        .map(|source| &source.native_identity),
                )
            } else {
                super::provider_log_acquisition::acquire_provider_log_batch(
                    &snapshot.session_id,
                    &snapshot.provider,
                    path,
                    provider_source_key,
                    Some(policy.next),
                    trust_source_from_start,
                )
            };
            let mut batch = batch.map_err(|error| AgentChatTranscriptFailure {
                stage: ChatTranscriptFailureStage::ProviderLogCapture,
                message: format!("provider-log acquisition failed: {error}"),
            })?;
            let _consumed_provider_log_bytes = batch.consumed_bytes;
            decorate_forward_provider_log_events(&mut batch.events, &snapshot.provider, path);
            if logging_enabled {
                attach_archive_bound_provider_log_retry_aliases(
                    &state.conversation_archive,
                    &context,
                    &mut batch.events,
                    path,
                )
                .map_err(|error| AgentChatTranscriptFailure {
                    stage: ChatTranscriptFailureStage::ArchiveWrite,
                    message: format!("conversation archive retry identity read failed: {error}"),
                })?;
            }
            // OpenCode's watcher can label a fallback message `opencode_db`,
            // but it does not carry the database session/path binding. Keep
            // the canonical DB projection in this incremental provider batch
            // so the existing archive writer persists the bound observation.
            let native_capture = logging_enabled
                .then(|| load_opencode_db_capture_for_snapshot(&snapshot))
                .flatten();
            let canonical_db_events = native_capture
                .as_ref()
                .map(|capture| capture.events.clone())
                .unwrap_or_else(|| load_opencode_db_chat_events_for_snapshot(&snapshot));
            if logging_enabled {
                batch.events.extend(canonical_db_events.iter().cloned());
            }
            state
                .conversation_archive
                .append_verified_provider_log_batch_with_native_messages(
                    context.clone(),
                    &batch.events,
                    batch.previous.as_ref(),
                    &batch.next,
                    batch.source_proof(),
                    native_capture.as_ref().map(|capture| &capture.authority),
                )
                .map_err(|error| AgentChatTranscriptFailure {
                    stage: ChatTranscriptFailureStage::ArchiveWrite,
                    message: format!("provider-log archive append failed: {error}"),
                })?;

            if !logging_enabled {
                // Preserve the existing live projection while the logging
                // policy suppresses new durable rows.
                batch.events.extend(canonical_db_events);
            }
            let background_stop = if !logging_enabled {
                CaptureStop::Disabled
            } else if batch.continue_immediately || source_index_pending {
                CaptureStop::More
            } else {
                CaptureStop::Observed {
                    status: batch.next.status.clone(),
                    reason: batch.next.reason.clone(),
                }
            };
            let mut result = collect_agent_chat_events_with_provider_events(
                &snapshot,
                batch.events,
                batch.continue_immediately,
            )
            .map_err(|message| AgentChatTranscriptFailure {
                stage: ChatTranscriptFailureStage::ProviderProjection,
                message,
            })?;
            recent_tags.apply(&mut result.events);
            result.background_stop = background_stop;
            let watch_only = result
                .events
                .iter()
                .filter(|event| event.metadata["provider_log"] != true)
                .cloned()
                .collect::<Vec<_>>();
            if logging_enabled {
                state
                    .conversation_archive
                    .append_chat_events_with_context(context, &watch_only)
                    .map_err(|error| AgentChatTranscriptFailure {
                        stage: ChatTranscriptFailureStage::ArchiveWrite,
                        message: format!("conversation archive watch append failed: {error}"),
                    })?;
            } else {
                state
                    .conversation_archive
                    .discard_agent_with_context(context, &watch_only)
                    .map_err(|error| AgentChatTranscriptFailure {
                        stage: ChatTranscriptFailureStage::ArchiveWrite,
                        message: format!("conversation archive disabled cutoff failed: {error}"),
                    })?;
            }
            return Ok(result);
        }
        if expected.is_some() {
            return Ok(ArchiveCaptureResult {
                events: Vec::new(),
                context,
                continue_immediately: false,
                background_stop: CaptureStop::Retired,
            });
        }
    }

    let native_capture = logging_enabled
        .then(|| load_opencode_db_capture_for_snapshot(&snapshot))
        .flatten();
    let mut result = collect_agent_chat_events_for_archive_bound(
        &snapshot,
        native_capture
            .as_ref()
            .map(|capture| capture.events.as_slice()),
    )
    .map_err(|message| AgentChatTranscriptFailure {
        stage: ChatTranscriptFailureStage::ProviderProjection,
        message,
    })?;
    recent_tags.apply(&mut result.events);
    result.background_stop = if logging_enabled {
        CaptureStop::Observed {
            status: "projection".to_string(),
            reason: Some("provider_log_unavailable".to_string()),
        }
    } else {
        CaptureStop::Disabled
    };
    if logging_enabled {
        let append = if let Some(capture) = native_capture.as_ref() {
            state
                .conversation_archive
                .append_chat_events_with_native_messages(
                    result.context.clone(),
                    &result.events,
                    &capture.authority,
                )
        } else {
            state
                .conversation_archive
                .append_chat_events_with_context(result.context.clone(), &result.events)
        };
        append.map_err(|error| AgentChatTranscriptFailure {
            stage: ChatTranscriptFailureStage::ArchiveWrite,
            message: format!("conversation archive append failed: {error}"),
        })?;
    } else {
        state
            .conversation_archive
            .discard_agent_with_context(result.context.clone(), &result.events)
            .map_err(|error| AgentChatTranscriptFailure {
                stage: ChatTranscriptFailureStage::ArchiveWrite,
                message: format!("conversation archive disabled cutoff failed: {error}"),
            })?;
    }

    Ok(result)
}

#[derive(Default)]
struct RecentWatchTags {
    rows: HashMap<String, (String, String)>,
    cursor: Option<String>,
}

impl RecentWatchTags {
    fn apply(&self, events: &mut [AgentChatEvent]) {
        for event in events {
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.remove("chat_recent_key");
                metadata.remove("chat_recent_version");
                // Retention/status changes can reuse a legacy mapper ordinal.
                // Qualify the exact actual Watch snapshot, not that ID alone.
                if let Some((key, version)) = self.rows.get(&event.id).filter(|_| {
                    self.cursor.is_some()
                        && metadata
                            .get("transcript_cursor")
                            .and_then(serde_json::Value::as_str)
                            == self.cursor.as_deref()
                }) {
                    metadata.insert("chat_recent_key".into(), serde_json::json!(key));
                    metadata.insert("chat_recent_version".into(), serde_json::json!(version));
                }
            }
        }
    }
}

fn publish_watch_recent(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
) -> std::io::Result<RecentWatchTags> {
    use crate::state::conversation_archive::chat_read;
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
    {
        return Err(std::io::Error::other(
            "recent admission waits for lifecycle boundary",
        ));
    }
    // A path discovered for this runtime does not describe every Watch row's
    // origin. Only actual file-derived rows need this exact parser receipt.
    let file_watch = if let Some(path) = &snapshot.log_path {
        let receipt = snapshot
            .watch_state
            .lock()
            .map_err(|_| std::io::Error::other("watch poisoned"))?
            .recent_file_source()
            .cloned();
        match receipt {
            Some((source, sequence)) if source.matches_path(path) && source.valid()? => {
                Some((source, sequence))
            }
            _ => None,
        }
    } else {
        None
    };
    let (current, runtime) =
        chat_read_snapshot_for_state(state, &snapshot.session_id).map_err(std::io::Error::other)?;
    if !Arc::ptr_eq(&current.watch_state, &snapshot.watch_state)
        || conversation_archive_context_from_snapshot(&current).provider_source_key
            != conversation_archive_context_from_snapshot(snapshot).provider_source_key
    {
        return Err(std::io::Error::other("recent producer retired"));
    }
    let owner = &state.conversation_archive.chat_projection;
    if owner.has_recent_native(snapshot, &runtime) {
        return Ok(RecentWatchTags::default());
    }
    let admission = owner.begin_recent_with_source(
        snapshot,
        &runtime,
        file_watch.as_ref().map(|(source, _)| source.clone()),
    )?;
    let token = &admission.0;
    let policy = crate::state::conversation_archive::recent_watch_policy(&snapshot.session_id)?;
    let (global_logging, _) =
        crate::utils::shell::recent_conversation_logging().map_err(std::io::Error::other)?;
    let disabled =
        effective_conversation_logging(global_logging, snapshot.agent_conversation_logging)
            == ConversationLoggingSetting::Disabled;
    let live_started = state
        .conversation_archive
        .live_conversation_started_at(&snapshot.session_id)?;
    let source_key = conversation_archive_context_from_snapshot(snapshot).provider_source_key;
    let mut headers = Vec::new();
    let mut tags = HashMap::new();
    let observed_cursor = {
        let watch = snapshot
            .watch_state
            .lock()
            .map_err(|_| std::io::Error::other("watch poisoned"))?;
        if !watch.codex_attachment_ready() {
            return Err(std::io::Error::other("recent watch attachment pending"));
        }
        let live = watch.live_chat_boundary().filter(|(started, _)| {
            live_started.as_ref() == Some(started)
                && (snapshot.resume_session.is_none()
                    || provider_log_source_is_fresh(
                        &snapshot.provider,
                        snapshot.resume_session.as_deref(),
                        snapshot.fresh_provider_session_id.as_deref(),
                    ))
        });
        if disabled && watch.live_chat_boundary().is_some() && live.is_none() {
            return Err(std::io::Error::other("recent live boundary retired"));
        }
        let transcript = WatchTranscript {
            cursor: watch.latest_cursor(),
            messages: Vec::new(),
            latest_text: String::new(),
            truncated: false,
            omitted_bytes: 0,
        };
        watch.visit_recent_transcript(
            &snapshot.current_status,
            |native_sequence, sequence, file_derived, message| {
                if file_derived
                    && !file_watch
                        .as_ref()
                        .is_some_and(|(_, bound)| native_sequence == *bound)
                {
                    return;
                }
                if message.role.len() > 64
                    || message.provider.len() > 32
                    || message.turn_id.as_ref().is_some_and(|id| id.len() > 256)
                    || message
                        .source
                        .as_ref()
                        .is_some_and(|source| source.len() > 128)
                    || message.provider_provenance.is_some()
                {
                    return;
                }
                if message.source.as_deref() == Some("opencode_db") {
                    return;
                }
                // Policy uses only the original mapper ID and metadata, never text.
                // Do not clone a potentially huge Watch body just to classify it.
                let shallow = WatchTranscriptMessage {
                    role: message.role.clone(),
                    text: String::new(),
                    provider: message.provider.clone(),
                    turn_id: message.turn_id.clone(),
                    source: message.source.clone(),
                    provider_provenance: None,
                };
                let mut event = message_event_from_transcript(
                    &snapshot.session_id,
                    &snapshot.provider,
                    sequence,
                    &shallow,
                    &transcript,
                );
                let durable_permitted = policy(&event, source_key.as_deref());
                let owned_live = disabled
                    && !file_derived
                    && live.is_some_and(|(_, floor)| native_sequence > *floor)
                    && message.source.as_deref() != Some("opencode_db");
                if event.provider != snapshot.provider
                    || (!durable_permitted && !owned_live)
                    || (disabled && live.is_some_and(|(_, floor)| native_sequence <= *floor))
                {
                    return;
                }
                let mut end = message.text.len().min(chat_read::PREVIEW_BYTES);
                while !message.text.is_char_boundary(end) {
                    end -= 1;
                }
                event.text = Some(message.text[..end].to_owned());
                let original_id = event.id.clone();
                let key = format!("{token}:watch:{native_sequence}");
                let version = crate::commands::chat_recent_seed::hash(key.as_bytes());
                tags.insert(original_id, (key.clone(), version.clone()));
                event.id = format!("preview:{token}:{native_sequence}");
                event.sequence = Some(native_sequence);
                event.metadata["chat_recent_key"] = serde_json::json!(key);
                event.metadata["chat_recent_version"] = serde_json::json!(version);
                headers.push(chat_read::header(&event));
            },
        );
        transcript.cursor
    };
    headers.reverse();
    let (after, after_runtime) =
        chat_read_snapshot_for_state(state, &snapshot.session_id).map_err(std::io::Error::other)?;
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
        || !Arc::ptr_eq(&runtime, &after_runtime)
        || !Arc::ptr_eq(&after.watch_state, &snapshot.watch_state)
        || !owner.publish_recent(&after, &runtime, &admission, headers)?
    {
        return Err(std::io::Error::other("recent producer publication retired"));
    }
    owner.note_recent_coverage(&admission, &tags)?;
    owner.note_recent_watch(&admission, observed_cursor.clone())?;
    Ok(RecentWatchTags {
        rows: tags,
        cursor: Some(observed_cursor),
    })
}

/// A bounded owner prelude shared with normal selection dispatch. It neither
/// waits for capture/archive work. Native source discovery stays in this
/// background/blocking producer, never in selection request dispatch.
pub(crate) fn publish_recent_for_state(
    state: &AppState,
    session_id: &str,
    expected_runtime: Option<&Arc<Mutex<String>>>,
) -> std::io::Result<()> {
    let (snapshot, runtime) =
        chat_read_snapshot_for_state(state, session_id).map_err(std::io::Error::other)?;
    if expected_runtime.is_some_and(|expected| !Arc::ptr_eq(expected, &runtime)) {
        return Err(std::io::Error::other("recent request runtime retired"));
    }
    if publish_database_recent(state, &snapshot, &runtime)? {
        return Ok(());
    }
    publish_watch_recent(state, &snapshot).map(|_| ())
}

fn publish_database_recent(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
    runtime: &Arc<Mutex<String>>,
) -> std::io::Result<bool> {
    use super::chat_recent_database;
    if snapshot.provider == "antigravity" {
        return publish_antigravity_recent(state, snapshot, runtime);
    }
    if snapshot.provider != "opencode" {
        return Ok(false);
    }
    let Some(native_session) = opencode_session_id(
        &snapshot.session_id,
        snapshot.resume_session.as_deref(),
        snapshot.fresh_provider_session_id.as_deref(),
    ) else {
        return Ok(false);
    };
    let Some(path) = opencode_database_path() else {
        return Ok(false);
    };
    let (logging, _) =
        crate::utils::shell::recent_conversation_logging().map_err(std::io::Error::other)?;
    if effective_conversation_logging(logging, snapshot.agent_conversation_logging)
        == ConversationLoggingSetting::Disabled
    {
        return Ok(false); // A database cannot use a current Watch's LIVE receipt.
    }
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
    {
        return Err(std::io::Error::other(
            "recent database admission waits for lifecycle boundary",
        ));
    }
    let owner = &state.conversation_archive.chat_projection;
    let source = chat_recent_database::Source::open(&path)?;
    let receipt = owner.begin_recent_with_source(snapshot, runtime, Some(source.clone()))?;
    let result = (|| {
        let policy =
            crate::state::conversation_archive::recent_native_time_policy(&snapshot.session_id)?;
        let source_key = conversation_archive_context_from_snapshot(snapshot).provider_source_key;
        let batch = chat_recent_database::opencode(&path, &native_session)?;
        if batch.source != source {
            return Err(std::io::Error::other(
                "recent database source changed during acquisition",
            ));
        }
        let mut headers = Vec::new();
        for row in batch.rows {
            let created = row
                .part
                .part_time_created
                .or(row.part.message_time_created)
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|time| time.to_rfc3339());
            // This native time/identity policy intentionally withholds an
            // unresolved legacy-ID cutoff. No guessed sequence reaches it.
            if !policy(source_key.as_deref(), created.as_deref()) {
                continue;
            }
            let identity = format!("preview:{}:{}", receipt.0, row.key);
            let Some(mut event) = opencode_db_part_to_chat_event_with_identity(
                &snapshot.session_id,
                &native_session,
                None,
                row.part,
                None,
                Some(identity),
            )
            .map_err(std::io::Error::other)?
            else {
                continue;
            };
            event.created_at = created;
            normalize_chat_event_visible_text(&mut event);
            if event.text.as_ref().is_none_or(|text| text.is_empty()) {
                continue;
            }
            event.metadata["chat_recent_key"] =
                serde_json::json!(format!("{}:{}", receipt.0, row.key));
            event.metadata["chat_recent_version"] = serde_json::json!(row.version);
            let mut header = crate::state::conversation_archive::chat_read::header(&event);
            // These are the actual bounded native keys, not reconstructed
            // canonical identities or a sequence in a truncated DB window.
            for key in ["opencode_session_id", "part_id"] {
                if let Some(value) = event.metadata.get(key) {
                    header.metadata[key] = value.clone();
                }
            }
            headers.push(header);
        }
        let (after, after_runtime) = chat_read_snapshot_for_state(state, &snapshot.session_id)
            .map_err(std::io::Error::other)?;
        if !Arc::ptr_eq(runtime, &after_runtime)
            || !Arc::ptr_eq(&snapshot.watch_state, &after.watch_state)
            || !state
                .conversation_capture_policy_lock
                .background_admission_open()
            || !owner.publish_recent(&after, runtime, &receipt, headers)?
        {
            return Err(std::io::Error::other("recent database publication retired"));
        }
        let _metrics = batch.metrics;
        Ok(true)
    })();
    if result.is_err() {
        owner.abandon_recent(&receipt);
    }
    result
}

fn publish_antigravity_recent(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
    runtime: &Arc<Mutex<String>>,
) -> std::io::Result<bool> {
    use super::chat_recent_database;
    let Some(session) = snapshot
        .resume_session
        .as_deref()
        .or(snapshot.fresh_provider_session_id.as_deref())
    else {
        return Ok(false);
    };
    let Some(path) = snapshot
        .log_path
        .as_deref()
        .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
    else {
        return Ok(false);
    };
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(session) {
        return Err(std::io::Error::other(
            "recent Antigravity source session mismatch",
        ));
    }
    let (logging, _) =
        crate::utils::shell::recent_conversation_logging().map_err(std::io::Error::other)?;
    if effective_conversation_logging(logging, snapshot.agent_conversation_logging)
        == ConversationLoggingSetting::Disabled
    {
        return Ok(false);
    }
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
    {
        return Err(std::io::Error::other(
            "recent Antigravity admission waits for lifecycle boundary",
        ));
    }
    let owner = &state.conversation_archive.chat_projection;
    let source = chat_recent_database::Source::open(path)?;
    let receipt = owner.begin_recent_with_source(snapshot, runtime, Some(source.clone()))?;
    let result = (|| {
        let policy =
            crate::state::conversation_archive::recent_native_time_policy(&snapshot.session_id)?;
        let source_key = conversation_archive_context_from_snapshot(snapshot).provider_source_key;
        let batch = chat_recent_database::antigravity(path, session)?;
        if batch.source != source {
            return Err(std::io::Error::other(
                "recent Antigravity source changed during acquisition",
            ));
        }
        let mut headers = Vec::new();
        for row in batch.rows {
            if !policy(source_key.as_deref(), None) {
                continue;
            }
            let id = format!("preview:{}:{}", receipt.0, row.key);
            let mut event = antigravity_database_event(
                &snapshot.session_id,
                &snapshot.provider,
                path,
                None,
                row.message,
                Some(id),
            )
            .map_err(std::io::Error::other)?;
            normalize_chat_event_visible_text(&mut event);
            if event.text.as_ref().is_none_or(|text| text.is_empty()) {
                continue;
            }
            event.metadata["chat_recent_key"] =
                serde_json::json!(format!("{}:{}", receipt.0, row.key));
            event.metadata["chat_recent_version"] = serde_json::json!(row.version);
            let mut header = crate::state::conversation_archive::chat_read::header(&event);
            header.metadata["step_index"] = event.metadata["step_index"].clone();
            headers.push(header);
        }
        let (after, after_runtime) = chat_read_snapshot_for_state(state, &snapshot.session_id)
            .map_err(std::io::Error::other)?;
        if !Arc::ptr_eq(runtime, &after_runtime)
            || !Arc::ptr_eq(&snapshot.watch_state, &after.watch_state)
            || !state
                .conversation_capture_policy_lock
                .background_admission_open()
            || !owner.publish_recent(&after, runtime, &receipt, headers)?
        {
            return Err(std::io::Error::other(
                "recent Antigravity publication retired",
            ));
        }
        let _metrics = batch.metrics;
        Ok(true)
    })();
    if result.is_err() {
        owner.abandon_recent(&receipt);
    }
    result
}

#[cfg(all(test, windows))]
pub(crate) fn capture_opencode_messages_for_test(
    path: &std::path::Path,
    agent: &str,
    session: &str,
) -> std::io::Result<(
    Vec<AgentChatEvent>,
    super::chat_recent_database::MessageAuthority,
)> {
    let capture = super::chat_recent_database::capture_opencode(path, agent, session)?;
    Ok((capture.events, capture.authority))
}

#[cfg(test)]
pub(crate) use tests::{
    archive_agent_chat_events_for_state, archive_agent_chat_events_until_stable_for_state,
    load_agent_chat_page_with_revalidation_for_state,
};

/// Drains the closing provider log for a lifecycle boundary (New Session, fresh
/// resume). Unlike the best-effort syncs above it does not queue behind them: it
/// waits only for the pass already running, and background syncs stand aside
/// until the whole drain finishes.
pub(crate) async fn archive_agent_chat_events_until_stable_for_lifecycle(
    state: &AppState,
    session_id: &str,
) -> Result<ArchiveCaptureResult, String> {
    let _boundary = state
        .conversation_capture_policy_lock
        .begin_lifecycle_boundary();
    loop {
        let result = archive_agent_chat_events_for_state_with_stage(
            state,
            session_id,
            CaptureLane::Lifecycle,
        )
        .await
        .map_err(|failure| failure.message)?;
        if !result.continue_immediately {
            return Ok(result);
        }
        tokio::task::yield_now().await;
    }
}

fn append_only_provider_log_path(snapshot: &AgentArchiveCaptureSnapshot) -> Option<&Path> {
    let path = snapshot.log_path.as_deref()?;
    if provider_log_path_is_cleared(
        &snapshot.provider,
        path,
        &snapshot.cleared_provider_sessions,
    ) || (snapshot.provider == "antigravity"
        && path.extension().is_some_and(|extension| extension == "db"))
    {
        None
    } else {
        Some(path)
    }
}

/// Build an incarnation-bound request without reading or seeding the archive.
pub(crate) async fn background_capture_request(
    state: &AppState,
    session_id: &str,
) -> Result<Option<CaptureRequest>, String> {
    let incarnation = match state.agents.lock().await.get(session_id) {
        Some(agent) => agent.current_status.clone(),
        None => return Ok(None),
    };
    let snapshot =
        agent_archive_capture_snapshot_bound(state, session_id, Some(&incarnation)).await?;
    // An absent source is bound watch-only capture. Observation failure is a
    // bound retry request, never permission for unbound provider acquisition.
    let source = snapshot
        .log_path
        .as_deref()
        .and_then(|path| super::provider_log_acquisition::observe_provider_log_source(path).ok());
    let global_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    Ok(Some(CaptureRequest {
        session_id: snapshot.session_id,
        incarnation,
        provider: snapshot.provider,
        conversation: snapshot
            .resume_session
            .or(snapshot.fresh_provider_session_id),
        source_path: snapshot.log_path,
        source,
        logging_enabled: effective_conversation_logging(
            global_logging,
            snapshot.agent_conversation_logging,
        ) == ConversationLoggingSetting::Enabled,
    }))
}

/// Each bounded pass revalidates after the policy gate. A disabled policy or
/// pending/invalid acquisition state suspends; none is inferred from a boolean.
pub(crate) async fn archive_background_capture_pass(
    state: &AppState,
    request: &CaptureRequest,
) -> Result<CaptureStop, String> {
    match archive_agent_chat_events_bound(
        state,
        &request.session_id,
        CaptureLane::Background,
        Some(request),
    )
    .await
    {
        Ok(result) => {
            if result.background_stop != CaptureStop::Disabled
                && result.background_stop != CaptureStop::Retired
            {
                state
                    .conversation_archive
                    .flush_deferred_chat_summaries(&result.context)
                    .map_err(|_| "chat summary maintenance failed".to_string())?;
            }
            let pending = state
                .conversation_archive
                .chat_projection
                .advance(&request.session_id)
                .map_err(|_| "chat projection checkpoint failed".to_string())?;
            if pending
                && result.background_stop != CaptureStop::Disabled
                && result.background_stop != CaptureStop::Retired
            {
                Ok(CaptureStop::More)
            } else {
                Ok(result.background_stop)
            }
        }
        Err(failure) if failure.stage == ChatTranscriptFailureStage::AgentSnapshot => {
            Ok(CaptureStop::Retired)
        }
        Err(failure) => Err(failure.message),
    }
}

pub(crate) fn record_provider_log_policy_for_snapshot(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
    logging_enabled: bool,
) -> Result<(), String> {
    let context = conversation_archive_context_from_snapshot(snapshot);
    let Some(provider_source_key) = context.provider_source_key.as_deref() else {
        return Ok(());
    };
    let Some(path) = append_only_provider_log_path(snapshot) else {
        if logging_enabled {
            state
                .conversation_archive
                .close_agent_capture_disabled_window(context)
                .map_err(|error| {
                    format!("conversation archive disabled window close failed: {error}")
                })?;
        }
        return Ok(());
    };
    let trust_source_from_start = provider_log_source_is_fresh(
        &snapshot.provider,
        snapshot.resume_session.as_deref(),
        snapshot.fresh_provider_session_id.as_deref(),
    );
    let previous = state
        .conversation_archive
        .provider_log_capture_state(&snapshot.session_id, provider_source_key)
        .map_err(|error| format!("provider-log capture state read failed: {error}"))?;
    let policy = super::provider_log_acquisition::observe_provider_log_policy_with_initial_absence(
        path,
        provider_source_key,
        previous.clone(),
        logging_enabled,
        trust_source_from_start,
    )
    .map_err(|error| format!("provider-log policy observation failed: {error}"))?;
    let Some(policy) = policy else {
        if logging_enabled {
            state
                .conversation_archive
                .close_agent_capture_disabled_window(context)
                .map_err(|error| {
                    format!("conversation archive disabled window close failed: {error}")
                })?;
        }
        return Ok(());
    };
    if previous.as_ref() != Some(&policy.next) {
        state
            .conversation_archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &[],
                previous.as_ref(),
                &policy.next,
            )
            .map_err(|error| format!("provider-log policy commit failed: {error}"))?;
    }
    if logging_enabled {
        state
            .conversation_archive
            .close_agent_capture_disabled_window(context)
            .map_err(|error| {
                format!("conversation archive disabled window close failed: {error}")
            })?;
    }
    Ok(())
}

pub(crate) fn decorate_forward_provider_log_events(
    events: &mut [AgentChatEvent],
    provider: &str,
    path: &Path,
) {
    let native_session = archive_identity::native_log_session(path, provider);
    for event in events.iter_mut() {
        let raw_line = event
            .metadata
            .as_object_mut()
            .and_then(|metadata| metadata.remove(PROVIDER_RAW_LINE_METADATA_KEY))
            .and_then(|value| value.as_str().map(str::to_string));
        set_metadata(&mut event.metadata, "provider_log", true);
        set_metadata(&mut event.metadata, "log_source", "active_agent_log_path");
        set_metadata(
            &mut event.metadata,
            "log_path",
            path.to_string_lossy().to_string(),
        );
        archive_identity::capture_legacy_identity(event, path, raw_line.as_deref());
        if let Some(session) = &native_session {
            set_metadata(&mut event.metadata, "provider_session_id", session.as_str());
        }
        if let Some(reference) = event.metadata["chat_source_ref"]
            .as_str()
            .map(str::to_owned)
        {
            if provider.eq_ignore_ascii_case("claude") {
                if let Some(raw_line) = raw_line.as_deref() {
                    if let Ok(raw) = serde_json::from_str::<serde_json::Value>(raw_line) {
                        if let Some(uuid) = raw["uuid"].as_str().filter(|uuid| !uuid.is_empty()) {
                            let canonical =
                                stable_provider_log_event_id_from_raw_line(event, path, raw_line);
                            set_metadata(
                                &mut event.metadata,
                                "chat_compatibility_raw_id",
                                canonical,
                            );
                            set_metadata(
                                &mut event.metadata,
                                "chat_compatibility_native_uuid",
                                uuid,
                            );
                        }
                    }
                }
            }
            event.id = reference;
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.remove("legacy_event_ids");
            }
            continue;
        }
        if provider.eq_ignore_ascii_case("claude") {
            if let Some(raw_line) = raw_line.as_deref() {
                let legacy_id = claude_legacy_provider_log_event_id(event, path, raw_line);
                event.id = stable_provider_log_event_id_from_raw_line(event, path, raw_line);
                if event.id != legacy_id && can_alias_legacy_provider_log_event_id(event) {
                    set_metadata(
                        &mut event.metadata,
                        "legacy_event_ids",
                        serde_json::json!([legacy_id]),
                    );
                }
            } else {
                event.id = stable_provider_log_event_id(event, path);
            }
        } else {
            event.id = stable_provider_log_event_id(event, path);
        }
    }
    attach_incremental_legacy_aliases(events, provider, path);
}

fn attach_archive_bound_provider_log_retry_aliases(
    archive: &crate::state::conversation_archive::ConversationArchiveState,
    context: &ConversationArchiveContext,
    events: &mut [AgentChatEvent],
    path: &Path,
) -> std::io::Result<()> {
    if !archive_identity::has_unaliased_archive_bound_legacy_row_candidate(events, path) {
        return Ok(());
    }
    let archived_events = archive.chat_events_for_capture(context)?;
    archive_identity::attach_archive_bound_legacy_row_aliases(events, path, &archived_events);
    Ok(())
}

fn attach_incremental_legacy_aliases(events: &mut [AgentChatEvent], provider: &str, path: &Path) {
    if provider.eq_ignore_ascii_case("claude")
        || !events.iter().any(|event| {
            requires_provider_log_row_identity(event)
                && event.metadata[PROVIDER_LOG_ROW_OFFSET_METADATA_KEY]
                    .as_u64()
                    .is_some()
        })
    {
        return;
    }
    let Some(session_id) = events.first().map(|event| event.session_id.as_str()) else {
        return;
    };
    let snapshot = load_provider_log_chat_events(session_id, provider, Some(path), &[]);
    let aliases_by_id = snapshot
        .into_iter()
        .filter_map(|event| {
            event
                .metadata
                .get("legacy_event_ids")
                .and_then(serde_json::Value::as_array)
                .filter(|aliases| !aliases.is_empty())
                .map(|aliases| (event.id, aliases.clone()))
        })
        .collect::<HashMap<_, _>>();
    for event in events {
        let Some(aliases) = aliases_by_id.get(&event.id) else {
            continue;
        };
        let Some(metadata) = event.metadata.as_object_mut() else {
            continue;
        };
        let existing = metadata
            .entry("legacy_event_ids")
            .or_insert_with(|| serde_json::json!([]));
        let Some(existing) = existing.as_array_mut() else {
            continue;
        };
        for alias in aliases {
            if alias.as_str().is_some_and(|alias| {
                !existing
                    .iter()
                    .any(|current| current.as_str() == Some(alias))
            }) {
                existing.push(alias.clone());
            }
        }
    }
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
    let Ok((content, row_offsets)) = read_provider_log_tail(path) else {
        return Vec::new();
    };

    let lines = content.lines().collect::<Vec<_>>();
    let complete = std::fs::metadata(path).is_ok_and(|meta| {
        meta.len() == content.len() as u64 && meta.len() <= PROVIDER_LOG_TAIL_BYTES
    });
    let mut events = normalize_chat_lines(session_id, provider, lines.iter());
    for event in &mut events {
        set_metadata(&mut event.metadata, "provider_log", true);
        set_metadata(&mut event.metadata, "log_source", "active_agent_log_path");
        set_metadata(
            &mut event.metadata,
            "log_path",
            path.to_string_lossy().to_string(),
        );
        if complete && matches!(provider, "codex" | "pi") {
            if let Some(sequence) = event.sequence {
                set_metadata(&mut event.metadata, "chat_legacy_source_sequence", sequence);
            }
        }
    }
    attach_provider_log_row_offsets(&mut events, &row_offsets, 1);
    for event in &mut events {
        if provider.eq_ignore_ascii_case("claude") {
            let raw_line = event
                .sequence
                .and_then(|sequence| sequence.checked_sub(1))
                .and_then(|index| lines.get(index as usize))
                .copied();
            if let Some(raw_line) = raw_line {
                let legacy_id = claude_legacy_provider_log_event_id(event, path, raw_line);
                event.id = stable_provider_log_event_id_from_raw_line(event, path, raw_line);
                if event.id != legacy_id && can_alias_legacy_provider_log_event_id(event) {
                    set_metadata(
                        &mut event.metadata,
                        "legacy_event_ids",
                        serde_json::json!([legacy_id]),
                    );
                }
            } else {
                event.id = stable_provider_log_event_id(event, path);
            }
        } else {
            event.id = stable_provider_log_event_id(event, path);
        }
    }
    // The bridge is available only for a complete, session-headed snapshot.
    // A bounded tail may use persisted aliases but cannot invent new ones.
    archive_identity::attach_native_legacy_aliases(&mut events, path, &content, complete);
    archive_identity::attach_unique_legacy_row_aliases(&mut events, path, complete);
    events
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
    legacy_provider_log_event_id(&legacy_event, path)
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
    let stable_id = format!(
        "{}:provider_log:{}",
        event.session_id,
        hex_prefix(hash.finalize().as_slice(), 16)
    );
    if requires_provider_log_row_identity(event) {
        with_provider_log_row_identity(event, stable_id)
    } else {
        stable_id
    }
}

fn load_antigravity_database_chat_events(
    session_id: &str,
    provider: &str,
    path: &Path,
) -> Vec<AgentChatEvent> {
    let Ok(messages) =
        AntigravityProvider::conversation_native_messages_from_database_since(path, None)
    else {
        return Vec::new();
    };
    let native_source = super::chat_recent_database::Source::open(path).ok();
    let native_session = path.file_stem().and_then(|stem| stem.to_str());

    let mut events: Vec<_> = messages
        .into_iter()
        .enumerate()
        .filter_map(|(index, native)| {
            let step = native.message.step_index;
            let mut event = antigravity_database_event(
                session_id,
                provider,
                path,
                Some(index as u64 + 1),
                native.message,
                None,
            )
            .ok()?;
            if let (Some(source), Some(session)) = (&native_source, native_session) {
                event.metadata["chat_recent_native_key"] =
                    serde_json::json!(source.key("antigravity", session, &step.to_string()));
                event.metadata["chat_recent_native_version"] = serde_json::json!(native.version);
            }
            Some(event)
        })
        .collect();
    if let Ok(tools) =
        crate::providers::antigravity::chat_tools::load_tools(session_id, provider, path)
    {
        events.extend(tools);
    }
    // Stable sort keeps a planner's existing message before its wire-ordered
    // calls. Message IDs were assigned before this projection and stay intact.
    events.sort_by_key(|event| event.metadata["step_index"].as_u64().unwrap_or_default());
    for (index, event) in events.iter_mut().enumerate() {
        event.sequence = Some(index as u64 + 1);
    }
    events
}

fn antigravity_database_event(
    session_id: &str,
    provider: &str,
    path: &Path,
    sequence: Option<u64>,
    message: crate::providers::antigravity::AntigravityConversationMessage,
    native_preview_id: Option<String>,
) -> Result<AgentChatEvent, String> {
    let mut event = AgentChatEvent {
        id: String::new(),
        session_id: session_id.to_owned(),
        provider: provider.to_owned(),
        kind: AgentChatEventKind::Message,
        role: Some(message.role.clone()),
        text: Some(message.text),
        title: None,
        status: None,
        turn_id: Some(message.step_index.to_string()),
        source: Some("conversation_database".into()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence,
        metadata: serde_json::json!({ "provider_log": true, "log_source": "antigravity_conversation_database",
            "log_path": path.to_string_lossy(), "step_index": message.step_index }),
    };
    // Preserve the full producer's historical identity before role correction.
    event.id = match (native_preview_id, sequence) {
        (Some(identity), _) => identity,
        (None, Some(_)) => stable_provider_log_event_id(&event, path),
        (None, None) => return Err("Antigravity native identity is unavailable".into()),
    };
    if message.role == AgentChatRole::User && message.source.is_some_and(|source| source != 4) {
        event.role = Some(AgentChatRole::System);
    }
    if let Some(source) = message.source {
        event.metadata["provider_step_source"] = serde_json::json!(source);
    }
    if message.role == AgentChatRole::User {
        let is_request = message.source.is_none_or(|source| source == 4);
        event.metadata["input_origin"] = serde_json::json!(if is_request {
            "human_input"
        } else {
            "provider_internal"
        });
        event.metadata["input_purpose"] =
            serde_json::json!(if is_request { "request" } else { "internal" });
        event.metadata["context_observation"] = serde_json::json!("unreported");
        if is_request {
            event.metadata["request_root_id"] = serde_json::json!(&event.id);
        }
    }
    Ok(event)
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

fn read_provider_log_tail(path: &Path) -> std::io::Result<(String, Vec<u64>)> {
    read_provider_log_tail_with_limit_and_offset(path, PROVIDER_LOG_TAIL_BYTES)
}

fn read_provider_log_tail_with_limit_and_offset(
    path: &Path,
    tail_bytes: u64,
) -> std::io::Result<(String, Vec<u64>)> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len <= tail_bytes {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        let row_offsets = provider_log_row_offsets(content.as_bytes(), 0);
        return Ok((content, row_offsets));
    }

    let start = file_len.saturating_sub(tail_bytes);
    let read_start = start.saturating_sub(1);
    file.seek(SeekFrom::Start(read_start))?;
    let mut bytes = Vec::with_capacity((file_len - read_start) as usize);
    file.read_to_end(&mut bytes)?;
    let (content_bytes, content_start_offset) =
        if read_start < start && bytes.first() == Some(&b'\n') {
            (&bytes[1..], start)
        } else {
            match bytes.iter().position(|byte| *byte == b'\n') {
                Some(index) => (
                    &bytes[index + 1..],
                    read_start.saturating_add(index as u64 + 1),
                ),
                None => (&bytes[bytes.len()..], file_len),
            }
        };
    let content = String::from_utf8_lossy(content_bytes).to_string();
    let row_offsets = provider_log_row_offsets(content_bytes, content_start_offset);
    Ok((content, row_offsets))
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

fn load_opencode_db_chat_events_for_snapshot(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> Vec<AgentChatEvent> {
    if snapshot.provider != "opencode" {
        return Vec::new();
    }
    load_opencode_db_chat_events(
        &snapshot.session_id,
        opencode_session_id(
            &snapshot.session_id,
            snapshot.resume_session.as_deref(),
            snapshot.fresh_provider_session_id.as_deref(),
        )
        .as_deref(),
    )
}

fn load_opencode_db_capture_for_snapshot(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> Option<super::chat_recent_database::FullOpenCodeCapture> {
    if snapshot.provider != "opencode" {
        return None;
    }
    let session = opencode_session_id(
        &snapshot.session_id,
        snapshot.resume_session.as_deref(),
        snapshot.fresh_provider_session_id.as_deref(),
    )?;
    super::chat_recent_database::capture_opencode(
        &opencode_database_path()?,
        &snapshot.session_id,
        &session,
    )
    .ok()
}

fn load_opencode_db_chat_events_from_db(
    db_path: &Path,
    wardian_session_id: &str,
    opencode_session_id: &str,
) -> Result<Vec<AgentChatEvent>, String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| err.to_string())?;
    let recent_source = super::chat_recent_database::Source::open(db_path).ok();
    project_opencode_database_connection(
        &conn,
        db_path,
        wardian_session_id,
        opencode_session_id,
        recent_source.as_ref(),
    )
}

pub(super) fn project_opencode_database_connection(
    conn: &rusqlite::Connection,
    db_path: &Path,
    wardian_session_id: &str,
    opencode_session_id: &str,
    recent_source: Option<&super::chat_recent_database::Source>,
) -> Result<Vec<AgentChatEvent>, String> {
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
    let mut text_sequence = 0;
    for row in rows {
        let row = row.map_err(|err| err.to_string())?;
        let recent_binding = recent_source.and_then(|source| {
            super::chat_recent_database::opencode_part_binding(source, opencode_session_id, &row)
                .ok()
        });
        events.extend(opencode_tools::project(
            wardian_session_id,
            opencode_session_id,
            &row,
            request_root_id.as_deref(),
            db_path,
            events.len() as u64 + 1,
        ));
        let Some(event) = opencode_db_part_to_chat_event(
            wardian_session_id,
            opencode_session_id,
            text_sequence + 1,
            row,
            request_root_id.as_deref(),
        )?
        else {
            continue;
        };
        let mut event = event;
        if let Some((key, version)) = recent_binding {
            event.metadata["chat_recent_native_key"] = serde_json::json!(key);
            event.metadata["chat_recent_native_version"] = serde_json::json!(version);
        }
        // Keep legacy text IDs stable when previously omitted tool parts appear.
        text_sequence += 1;
        event.sequence = Some(events.len() as u64 + 1);
        event.created_at = event.metadata["part_time_created"]
            .as_i64()
            .or_else(|| event.metadata["message_time_created"].as_i64())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|time| time.to_rfc3339());
        // `source_path` is part of the archive source-record contract and is
        // retained when events are replayed after a restart.
        event.metadata["source_path"] = serde_json::json!(db_path.to_string_lossy());
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

pub(super) struct OpencodeDbPart {
    pub(super) part_id: String,
    pub(super) part_data: String,
    pub(super) part_time_created: Option<i64>,
    pub(super) message_id: String,
    pub(super) message_data: String,
    pub(super) message_time_created: Option<i64>,
}

fn opencode_db_part_to_chat_event(
    wardian_session_id: &str,
    opencode_session_id: &str,
    sequence: u64,
    row: OpencodeDbPart,
    request_root_id: Option<&str>,
) -> Result<Option<AgentChatEvent>, String> {
    opencode_db_part_to_chat_event_with_identity(
        wardian_session_id,
        opencode_session_id,
        Some(sequence),
        row,
        request_root_id,
        None,
    )
}

pub(super) fn opencode_db_part_to_chat_event_with_identity(
    wardian_session_id: &str,
    opencode_session_id: &str,
    sequence: Option<u64>,
    row: OpencodeDbPart,
    request_root_id: Option<&str>,
    native_preview_id: Option<String>,
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
        id: match (native_preview_id, sequence) {
            (Some(identity), _) => identity,
            (None, Some(sequence)) => event_id(
                wardian_session_id,
                sequence,
                &format!("opencode_db:{}", row.part_id),
            ),
            (None, None) => return Err("native database identity is unavailable".into()),
        },
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
        sequence,
        metadata,
    }))
}

fn opencode_session_id(
    wardian_session_id: &str,
    resume_session: Option<&str>,
    fresh_provider_session_id: Option<&str>,
) -> Option<String> {
    resume_session
        .map(str::trim)
        .filter(|session| session.starts_with("ses_"))
        .or_else(|| {
            fresh_provider_session_id
                .map(str::trim)
                .filter(|session| session.starts_with("ses_"))
        })
        .or_else(|| {
            let session = wardian_session_id.trim();
            session.starts_with("ses_").then_some(session)
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
    let mut watch_events = watch_events;
    let mut provider_events = provider_events;
    coalesce_codex_watch_observations(&mut watch_events, &mut provider_events);
    // Mirror matching must compare visible text, not provider wrapper payloads.
    for event in &mut provider_events {
        normalize_chat_event_visible_text(event);
    }
    for event in &mut watch_events {
        normalize_chat_event_visible_text(event);
    }
    coalesce_codex_stream_completion_observations(&mut provider_events);
    coalesce_codex_stream_completion_observations(&mut watch_events);

    let mut seen = HashSet::new();
    let mut archived_event_ids = HashMap::new();
    let mut live_event_ids = HashMap::new();
    let mut provider_message_text_seen = HashSet::new();
    let mut provider_message_indexes_by_text = HashMap::new();
    let mut merged = Vec::with_capacity(watch_events.len() + provider_events.len());

    for event in provider_events {
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
                            let mut replacement = event;
                            retain_provider_observation_ids(
                                &mut replacement,
                                &merged[*existing_index],
                            );
                            merged[*existing_index] = replacement;
                        } else {
                            retain_provider_observation_ids(&mut merged[*existing_index], &event);
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

    for event in watch_events {
        if is_cross_source_archive_duplicate(&event, &archived_event_ids, &live_event_ids) {
            continue;
        }
        remember_archive_event_id(&event, &mut archived_event_ids, &mut live_event_ids);
        let identified_codex_assistant = event.provider == "codex"
            && event.role == Some(AgentChatRole::Assistant)
            && (event.metadata["provider_log"] == true || event.turn_id.is_some());
        if !identified_codex_assistant
            && chat_message_text_key(&event)
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

    canonicalize_provider_input_projection(&mut merged);
    for (index, event) in merged.iter_mut().enumerate() {
        event.sequence = Some(index as u64 + 1);
    }

    merged
}

/// Resolve stream/completion mirrors against the complete candidate set before
/// inserting rows. A greedy text index cannot distinguish two native messages.
fn coalesce_codex_stream_completion_observations(events: &mut Vec<AgentChatEvent>) {
    let mut removed = HashSet::new();
    for stream_index in 0..events.len() {
        if events[stream_index].source.as_deref() != Some("event_msg") {
            continue;
        }
        let Some(message_key) = chat_message_text_key(&events[stream_index]) else {
            continue;
        };
        let matches = (0..events.len())
            .filter(|&index| {
                archive_identity::is_codex_stream_completion_pair(
                    &events[stream_index],
                    &events[index],
                ) && chat_message_text_key(&events[index]).as_ref() == Some(&message_key)
            })
            .collect::<Vec<_>>();
        let identities = matches
            .iter()
            .map(|&index| events[index].id.as_str())
            .collect::<HashSet<_>>();
        if identities.len() != 1 {
            continue;
        }
        let completed_index = matches[0];
        let mirrors = events
            .iter()
            .filter(|candidate| {
                archive_identity::is_codex_stream_completion_pair(
                    candidate,
                    &events[completed_index],
                ) && chat_message_text_key(candidate)
                    == chat_message_text_key(&events[completed_index])
            })
            .map(|event| event.id.as_str())
            .collect::<HashSet<_>>();
        if mirrors.len() != 1 {
            continue;
        }
        let duplicate = events[stream_index].clone();
        for completed_index in matches {
            retain_provider_observation_ids(&mut events[completed_index], &duplicate);
        }
        removed.insert(stream_index);
    }
    if !removed.is_empty() {
        *events = events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

/// Reconciles a Codex watcher observation with its provider-log copy before
/// the older event/response mirror rule runs. The watcher carries the native
/// message ID in `turn_id`; the provider turn remains metadata. Every binding
/// key is required, so equal text cannot establish a duplicate.
fn coalesce_codex_watch_observations(
    watch_events: &mut Vec<AgentChatEvent>,
    provider_events: &mut [AgentChatEvent],
) {
    let mut removed = HashSet::new();
    for (watch_index, watch_event) in watch_events.iter().enumerate() {
        if !is_codex_watch_observation(watch_event) {
            continue;
        }
        let native_matches = provider_events
            .iter()
            .enumerate()
            .filter(|(_, native_event)| {
                is_codex_native_observation(native_event)
                    && codex_watch_native_observation_matches(watch_event, native_event)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let native_ids = native_matches
            .iter()
            .map(|&index| provider_events[index].id.as_str())
            .collect::<HashSet<_>>();
        if native_ids.len() != 1 {
            continue;
        }
        let native_index = native_matches[0];
        let watch_ids = watch_events
            .iter()
            .filter(|candidate| {
                is_codex_watch_observation(candidate)
                    && codex_watch_native_observation_matches(
                        candidate,
                        &provider_events[native_index],
                    )
            })
            .map(|event| event.id.as_str())
            .collect::<HashSet<_>>();
        if watch_ids.len() != 1 {
            continue;
        }

        let mut canonical = provider_events[native_index].clone();
        retain_provider_observation_ids(&mut canonical, watch_event);
        provider_events[native_index] = canonical;
        removed.insert(watch_index);
    }

    if !removed.is_empty() {
        *watch_events = watch_events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

fn coalesce_codex_watch_observations_in_place(events: &mut Vec<AgentChatEvent>) {
    let mut removed = HashSet::new();
    for watch_index in 0..events.len() {
        if removed.contains(&watch_index) || !is_codex_watch_observation(&events[watch_index]) {
            continue;
        }
        let native_matches = (0..events.len())
            .filter(|&native_index| {
                native_index != watch_index
                    && !removed.contains(&native_index)
                    && is_codex_native_observation(&events[native_index])
                    && codex_watch_native_observation_matches(
                        &events[watch_index],
                        &events[native_index],
                    )
            })
            .collect::<Vec<_>>();
        let native_ids = native_matches
            .iter()
            .map(|&index| events[index].id.as_str())
            .collect::<HashSet<_>>();
        if native_ids.len() != 1 {
            continue;
        }
        let native_index = native_matches[0];
        let watch_ids = events
            .iter()
            .filter(|candidate| {
                is_codex_watch_observation(candidate)
                    && codex_watch_native_observation_matches(candidate, &events[native_index])
            })
            .map(|event| event.id.as_str())
            .collect::<HashSet<_>>();
        if watch_ids.len() != 1 {
            continue;
        }

        let mut canonical = events[native_index].clone();
        retain_provider_observation_ids(&mut canonical, &events[watch_index]);
        events[native_index] = canonical;
        removed.insert(watch_index);
    }

    if !removed.is_empty() {
        *events = events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

fn coalesce_codex_watch_observations_across(
    current_events: &mut Vec<AgentChatEvent>,
    archived_events: &mut Vec<AgentChatEvent>,
) {
    coalesce_codex_watch_observations(current_events, archived_events);
    coalesce_codex_watch_observations(archived_events, current_events);
    coalesce_codex_watch_observations_in_place(current_events);
    coalesce_codex_watch_observations_in_place(archived_events);
}

fn is_codex_watch_observation(event: &AgentChatEvent) -> bool {
    event.provider.eq_ignore_ascii_case("codex")
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && matches!(event.source.as_deref(), Some("event_msg" | "response_item"))
        && event.metadata.get("transcript_cursor").is_some()
}

fn is_codex_native_observation(event: &AgentChatEvent) -> bool {
    event.provider.eq_ignore_ascii_case("codex")
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && matches!(event.source.as_deref(), Some("event_msg" | "response_item"))
        && event.metadata["provider_log"] == true
        && event.metadata.get("transcript_cursor").is_none()
}

fn event_metadata_string<'a>(event: &'a AgentChatEvent, key: &str) -> Option<&'a str> {
    event
        .metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn codex_watch_native_observation_matches(watch: &AgentChatEvent, native: &AgentChatEvent) -> bool {
    if watch.session_id != native.session_id
        || watch.provider != native.provider
        || watch.source != native.source
        || watch.turn_id != native.turn_id
    {
        return false;
    }
    for key in ["provider_phase", "request_root_id", "source_path"] {
        if matches!(
            (event_metadata_string(watch, key), event_metadata_string(native, key)),
            (Some(left), Some(right)) if left != right
        ) {
            return false;
        }
    }
    let same_path = event_metadata_string(watch, "log_path")
        .zip(event_metadata_string(native, "log_path"))
        .is_some_and(|(watch_path, native_path)| watch_path == native_path);
    let same_provider_turn = event_metadata_string(watch, "provider_turn_id")
        .zip(event_metadata_string(native, "provider_turn_id"))
        .is_some_and(|(watch_turn, native_turn)| watch_turn == native_turn);
    let same_provider_session = match (
        event_metadata_string(watch, "provider_session_id"),
        event_metadata_string(native, "provider_session_id"),
    ) {
        (Some(watch_session), Some(native_session)) => watch_session == native_session,
        _ => true,
    };
    same_path && same_provider_turn && same_provider_session
}

/// Projects only a source-bound Codex user mirror pair into one chat row.
fn canonicalize_provider_input_projection(events: &mut Vec<AgentChatEvent>) {
    let mut removed = HashSet::new();
    for mirror_index in 0..events.len() {
        if removed.contains(&mirror_index)
            || events[mirror_index].source.as_deref() != Some("event_msg")
        {
            continue;
        }
        let request_matches = (0..events.len())
            .filter(|&request_index| {
                request_index != mirror_index
                    && !removed.contains(&request_index)
                    && codex_user_mirror_pair(&events[mirror_index], &events[request_index])
            })
            .collect::<Vec<_>>();
        if request_matches.len() != 1 {
            continue;
        }
        let request_index = request_matches[0];
        let mirror_matches = (0..events.len())
            .filter(|&candidate_index| {
                candidate_index != request_index
                    && !removed.contains(&candidate_index)
                    && codex_user_mirror_pair(&events[request_index], &events[candidate_index])
            })
            .collect::<Vec<_>>();
        if mirror_matches.len() != 1 || mirror_matches[0] != mirror_index {
            continue;
        }

        let mut canonical = events[request_index].clone();
        retain_provider_observation_ids(&mut canonical, &events[mirror_index]);
        events[request_index] = canonical;
        removed.insert(mirror_index);
    }
    if !removed.is_empty() {
        *events = events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

fn retain_provider_observation_ids(canonical: &mut AgentChatEvent, duplicate: &AgentChatEvent) {
    let mut ids = provider_observation_ids(canonical);
    for id in provider_observation_ids(duplicate) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    canonical.metadata["provider_observation_ids"] = serde_json::json!(ids);

    if canonical.source.as_deref() == Some("response_item")
        && duplicate.source.as_deref() == Some("event_msg")
    {
        let mut roots = canonical.metadata["provider_mirror_request_root_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mirror_roots = duplicate.metadata["provider_mirror_request_root_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .chain(event_metadata_string(duplicate, "request_root_id"));
        for root in mirror_roots {
            if event_metadata_string(canonical, "request_root_id") != Some(root)
                && !roots.iter().any(|value| value.as_str() == Some(root))
            {
                roots.push(serde_json::json!(root));
            }
        }
        if !roots.is_empty() {
            canonical.metadata["provider_mirror_request_root_ids"] =
                serde_json::Value::Array(roots);
        }
    }
}

fn provider_observation_ids(event: &AgentChatEvent) -> Vec<String> {
    let mut ids = event
        .metadata
        .get("provider_observation_ids")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !ids.iter().any(|id| id == &event.id) {
        ids.push(event.id.clone());
    }
    ids
}

fn is_cross_source_archive_duplicate(
    event: &AgentChatEvent,
    archived_event_ids: &HashMap<String, Vec<AgentChatEvent>>,
    live_event_ids: &HashMap<String, Vec<AgentChatEvent>>,
) -> bool {
    let event_ids = event_identity_ids(event);
    let others = if event_is_archived(event) {
        live_event_ids
    } else {
        archived_event_ids
    };
    event_ids.iter().any(|id| {
        others.get(*id).is_some_and(|candidates| {
            candidates.iter().any(|other| {
                event.provider != "codex"
                    || event.kind != AgentChatEventKind::Message
                    || event.role != Some(AgentChatRole::Assistant)
                    || crate::state::conversation_archive::provenance::codex_observation_identity_matches(
                        event, other,
                    )
            })
        })
    })
}

fn remember_archive_event_id(
    event: &AgentChatEvent,
    archived_event_ids: &mut HashMap<String, Vec<AgentChatEvent>>,
    live_event_ids: &mut HashMap<String, Vec<AgentChatEvent>>,
) {
    let target = if event_is_archived(event) {
        archived_event_ids
    } else {
        live_event_ids
    };
    for id in event_identity_ids(event) {
        target.entry(id.to_owned()).or_default().push(event.clone());
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
    ids.extend(crate::state::conversation_archive::provenance::provider_observation_aliases(event));
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
    crate::state::conversation_archive::provenance::canonicalize_role(event);
}

fn should_collapse_provider_message_duplicate(
    existing: &AgentChatEvent,
    candidate: &AgentChatEvent,
) -> bool {
    if existing.metadata["provider_log"] == true && candidate.metadata["provider_log"] == true {
        return false; // Native records with distinct IDs are distinct observations.
    }
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

    if event.metadata["provider_log"] == true {
        return format!(
            "native|{}|{}|{}",
            event.session_id, event.provider, event.id
        );
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
    let source = message
        .source
        .clone()
        .or_else(|| Some("transcript".to_string()));
    let is_bound_native_watch = (message.provider.eq_ignore_ascii_case("codex")
        && matches!(source.as_deref(), Some("event_msg" | "response_item")))
        || (message.provider.eq_ignore_ascii_case("pi")
            && source.as_deref() == Some("session_jsonl"));
    if is_bound_native_watch {
        if let Some(provenance) = message.provider_provenance.as_ref().filter(|provenance| {
            !provenance.provider_session_id.trim().is_empty()
                && !provenance.source_path.trim().is_empty()
                && !provenance.provider_turn_id.trim().is_empty()
        }) {
            set_metadata(&mut metadata, "provider_log", true);
            set_metadata(
                &mut metadata,
                "provider_session_id",
                provenance.provider_session_id.clone(),
            );
            set_metadata(&mut metadata, "log_path", provenance.source_path.clone());
            set_metadata(
                &mut metadata,
                "provider_turn_id",
                provenance.provider_turn_id.clone(),
            );
        }
    }

    AgentChatEvent {
        id: event_id(
            session_id,
            sequence,
            source.as_deref().unwrap_or("transcript"),
        ),
        session_id: session_id.to_string(),
        provider,
        kind: AgentChatEventKind::Message,
        role,
        text: Some(message.text.clone()),
        title: None,
        status: None,
        turn_id: message.turn_id.clone(),
        source,
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

    pub(crate) async fn load_agent_chat_page_with_revalidation_for_state(
        state: &AppState,
        session_id: String,
        cursor: Option<String>,
        revision: Option<String>,
        detail_ref: Option<String>,
        revalidate_ids: Option<Vec<String>>,
    ) -> Result<wardian_core::models::chat::AgentChatPage, String> {
        load_agent_chat_page_with_limit_for_state(
            state,
            session_id,
            cursor,
            revision,
            detail_ref,
            revalidate_ids,
            None,
        )
        .await
    }

    pub(crate) async fn load_agent_chat_page_with_limit_for_state(
        state: &AppState,
        session_id: String,
        cursor: Option<String>,
        revision: Option<String>,
        detail_ref: Option<String>,
        revalidate_ids: Option<Vec<String>>,
        recent_limit: Option<usize>,
    ) -> Result<wardian_core::models::chat::AgentChatPage, String> {
        load_agent_chat_page_with_recent_for_state(
            state,
            session_id,
            cursor,
            revision,
            detail_ref,
            revalidate_ids,
            RecentChatRead {
                limit: recent_limit,
                preview: false,
            },
        )
        .await
    }

    #[test]
    fn recent_watch_canonical_binding_rejects_reused_legacy_id_from_another_cursor() {
        let message = WatchTranscriptMessage {
            role: "assistant".into(),
            text: "current".into(),
            provider: "mock".into(),
            turn_id: None,
            source: None,
            provider_provenance: None,
        };
        let original = WatchTranscript {
            cursor: "watch:01".into(),
            messages: Vec::new(),
            latest_text: String::new(),
            truncated: false,
            omitted_bytes: 0,
        };
        let mut actual = message_event_from_transcript("agent", "mock", 1, &message, &original);
        let mut other = message_event_from_transcript(
            "agent",
            "mock",
            1,
            &WatchTranscriptMessage {
                text: "different physical row".into(),
                ..message
            },
            &WatchTranscript {
                cursor: "watch:02".into(),
                ..original.clone()
            },
        );
        assert_eq!(
            actual.id, other.id,
            "the real mapper's legacy ordinal alone is insufficient"
        );
        let tags = RecentWatchTags {
            rows: HashMap::from([(
                actual.id.clone(),
                ("owned-native-key".into(), "actual-version".into()),
            )]),
            cursor: Some(original.cursor),
        };
        tags.apply(std::slice::from_mut(&mut actual));
        assert_eq!(actual.metadata["chat_recent_key"], "owned-native-key");
        other.metadata["chat_recent_key"] = serde_json::json!("forged-key");
        other.metadata["chat_recent_version"] = serde_json::json!("forged-version");
        tags.apply(std::slice::from_mut(&mut other));
        assert!(
            other.metadata.get("chat_recent_key").is_none()
                && other.metadata.get("chat_recent_version").is_none()
        );
    }

    pub(crate) async fn archive_agent_chat_events_for_state(
        state: &AppState,
        session_id: &str,
    ) -> Result<ArchiveCaptureResult, String> {
        archive_agent_chat_events_for_state_with_stage(state, session_id, CaptureLane::Background)
            .await
            .map_err(|failure| failure.message)
    }

    /// Drains consecutive bounded provider-log batches for owners that already
    /// run outside the UI request path. Each pass yields before reacquiring policy
    /// and per-agent gates so settings transitions and other agents can proceed.
    pub(crate) async fn archive_agent_chat_events_until_stable_for_state(
        state: &AppState,
        session_id: &str,
    ) -> Result<ArchiveCaptureResult, String> {
        loop {
            let result = archive_agent_chat_events_for_state(state, session_id).await?;
            if !result.continue_immediately {
                return Ok(result);
            }
            tokio::task::yield_now().await;
        }
    }

    struct WardianHomeGuard<'a> {
        _env_lock: &'a tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
    }

    impl<'a> WardianHomeGuard<'a> {
        fn set(env_lock: &'a tokio::sync::MutexGuard<'static, ()>, home: &std::path::Path) -> Self {
            let guard = Self {
                _env_lock: env_lock,
                previous_home: std::env::var_os("WARDIAN_HOME"),
            };
            std::env::set_var("WARDIAN_HOME", home);
            guard
        }
    }

    impl Drop for WardianHomeGuard<'_> {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(home) => std::env::set_var("WARDIAN_HOME", home),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    #[tokio::test]
    async fn wardian_home_guard_restores_previous_value_during_unwind() {
        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let original_home = std::env::var_os("WARDIAN_HOME");
        let sentinel_home = tempfile::tempdir().expect("sentinel home");
        let panic_home = tempfile::tempdir().expect("panic home");
        let sentinel_value = sentinel_home.path().as_os_str().to_owned();
        let original_guard = WardianHomeGuard::set(&env_lock, sentinel_home.path());

        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _panic_guard = WardianHomeGuard::set(&env_lock, panic_home.path());
            panic!("exercise WARDIAN_HOME restoration during unwinding");
        }));

        assert!(panic_result.is_err(), "the test panic must be caught");
        assert_eq!(std::env::var_os("WARDIAN_HOME"), Some(sentinel_value));

        drop(original_guard);
        assert_eq!(std::env::var_os("WARDIAN_HOME"), original_home);
    }

    #[test]
    fn launch_owned_fresh_identity_trusts_prefix_but_resume_does_not() {
        assert!(provider_log_source_is_fresh(
            "pi",
            None,
            Some("pi-fresh-session")
        ));
        assert!(provider_log_source_is_fresh(
            "pi",
            Some("pi-fresh-session"),
            Some("pi-fresh-session")
        ));
        assert!(!provider_log_source_is_fresh(
            "pi",
            Some("pi-resumed-session"),
            None
        ));
        assert!(!provider_log_source_is_fresh(
            "pi",
            Some("pi-resumed-session"),
            Some("pi-other-session")
        ));
        assert!(provider_log_source_is_fresh(
            "codex",
            Some("codex-session"),
            Some("codex-session")
        ));
        assert!(!provider_log_source_is_fresh(
            "codex",
            Some("codex-session"),
            None
        ));
        assert!(!provider_log_source_is_fresh(
            "codex",
            Some("codex-session"),
            Some("other-session")
        ));
        assert!(!provider_log_source_is_fresh(
            "codex",
            Some(""),
            Some("codex-session")
        ));
        assert!(!provider_log_source_is_fresh(
            "codex",
            Some("codex-session"),
            Some("")
        ));
        assert!(!provider_log_source_is_fresh(
            "opencode",
            Some("opencode-session"),
            Some("opencode-session")
        ));
    }

    #[test]
    fn mock_prefix_trust_requires_its_runtime_launch_identity() {
        for (resume, fresh, expected) in [
            (None, Some("owned"), true),
            (Some("owned"), Some("owned"), true),
            (Some(" owned "), Some(" owned "), true),
            (Some("owned"), None, false),
            (Some("owned"), Some("foreign"), false),
            (Some(""), Some("owned"), false),
            (Some("owned"), Some(""), false),
        ] {
            assert_eq!(
                provider_log_source_is_fresh("mock", resume, fresh),
                expected
            );
        }
        assert!(!provider_log_source_is_fresh(
            "gemini",
            Some("owned"),
            Some("owned")
        ));
    }

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

    #[tokio::test]
    async fn watch_archive_retry_survives_provider_event_growth() {
        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("isolated archive home");
        let _home_guard = WardianHomeGuard::set(&env_lock, temp.path());
        let watch_transcript = transcript(vec![WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "Completed the requested change.".to_string(),
            provider: "mock".to_string(),
            turn_id: None,
            source: Some("model".to_string()),
            provider_provenance: None,
        }]);
        let observation = message_event_from_transcript(
            "agent-1",
            "mock",
            6,
            &watch_transcript.messages[0],
            &watch_transcript,
        );
        let first = merge_chat_events(vec![observation.clone()], Vec::new());
        let mut provider_tool = observation.clone();
        provider_tool.id = "agent-1:provider:tool-1".to_string();
        provider_tool.kind = AgentChatEventKind::ToolCall;
        provider_tool.role = None;
        provider_tool.text = None;
        provider_tool.title = Some("Read".to_string());
        provider_tool.source = Some("tool_call".to_string());
        provider_tool.metadata = serde_json::json!({"cursor": "physical-tool-1"});
        let repeated = merge_chat_events(vec![observation], vec![provider_tool]);
        let same_message = repeated
            .iter()
            .find(|event| event.id == first[0].id)
            .expect("same watch observation remains present");
        assert_eq!(same_message.text, first[0].text);
        assert_ne!(
            same_message.sequence, first[0].sequence,
            "the presentation sequence changes when another event precedes it"
        );

        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        archive
            .append_chat_events("agent-1", &first)
            .expect("first observation publishes");
        archive
            .append_chat_events("agent-1", &repeated)
            .expect("reordering the same observation must not conflict with its published source");
        let archived = archive
            .chat_events_for_agent("agent-1")
            .expect("read archive");
        assert_eq!(
            archived
                .iter()
                .filter(|event| event.id == first[0].id)
                .count(),
            1
        );
    }

    #[test]
    fn projects_only_bound_codex_watch_messages_as_provider_observations() {
        let bound = WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "Codex answer".to_string(),
            provider: "codex".to_string(),
            turn_id: None,
            source: Some("event_msg".to_string()),
            provider_provenance: Some(wardian_core::control::WatchTranscriptProvenance {
                provider_session_id: "codex-session".to_string(),
                source_path: "codex-session.jsonl".to_string(),
                provider_turn_id: "turn-a".to_string(),
            }),
        };
        let bound_event = message_event_from_transcript(
            "agent-1",
            "codex",
            1,
            &bound,
            &transcript(vec![bound.clone()]),
        );

        assert_eq!(bound_event.turn_id, None);
        assert_eq!(bound_event.metadata["provider_log"], true);
        assert_eq!(bound_event.metadata["provider_session_id"], "codex-session");
        assert_eq!(bound_event.metadata["log_path"], "codex-session.jsonl");
        assert_eq!(bound_event.metadata["provider_turn_id"], "turn-a");

        let response = WatchTranscriptMessage {
            source: Some("response_item".to_string()),
            turn_id: Some("msg-native".to_string()),
            ..bound.clone()
        };
        let response_event = message_event_from_transcript(
            "agent-1",
            "codex",
            2,
            &response,
            &transcript(vec![response.clone()]),
        );
        assert_eq!(response_event.turn_id.as_deref(), Some("msg-native"));
        assert_eq!(response_event.metadata["provider_log"], true);
        assert_eq!(response_event.metadata["provider_turn_id"], "turn-a");

        let unbound = WatchTranscriptMessage {
            provider_provenance: None,
            ..bound
        };
        let unbound_event = message_event_from_transcript(
            "agent-1",
            "codex",
            2,
            &unbound,
            &transcript(vec![unbound.clone()]),
        );
        assert_ne!(unbound_event.metadata["provider_log"], true);
        assert!(unbound_event.metadata.get("provider_turn_id").is_none());
    }

    #[test]
    fn projects_bound_pi_watch_messages_as_provider_observations() {
        let bound = WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "Pi answer".to_string(),
            provider: "pi".to_string(),
            turn_id: Some("118bf261".to_string()),
            source: Some("session_jsonl".to_string()),
            provider_provenance: Some(wardian_core::control::WatchTranscriptProvenance {
                provider_session_id: "1f2a5d81-5ca6-46bf-b15a-78c6295d64b7".to_string(),
                source_path: "pi-session.jsonl".to_string(),
                provider_turn_id: "118bf261".to_string(),
            }),
        };
        let bound_event = message_event_from_transcript(
            "dac9e431-f775-4c77-8b8e-0a61c6dba9e4",
            "pi",
            3,
            &bound,
            &transcript(vec![bound.clone()]),
        );

        assert_eq!(bound_event.metadata["provider_log"], true);
        assert_eq!(
            bound_event.metadata["provider_session_id"],
            "1f2a5d81-5ca6-46bf-b15a-78c6295d64b7"
        );
        assert_eq!(bound_event.metadata["log_path"], "pi-session.jsonl");
        assert_eq!(bound_event.metadata["provider_turn_id"], "118bf261");

        let unbound = WatchTranscriptMessage {
            provider_provenance: None,
            ..bound
        };
        let unbound_event = message_event_from_transcript(
            "dac9e431-f775-4c77-8b8e-0a61c6dba9e4",
            "pi",
            3,
            &unbound,
            &transcript(vec![unbound.clone()]),
        );
        assert_ne!(unbound_event.metadata["provider_log"], true);
        assert!(unbound_event.metadata.get("provider_session_id").is_none());
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
            provider_provenance: None,
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
            provider_provenance: None,
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
            provider_provenance: None,
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

    #[tokio::test]
    async fn identityless_provider_retry_reconciles_pre_offset_archive_and_cursor() {
        use std::io::Write as _;

        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("isolated Wardian home");
        let _home = WardianHomeGuard::set(&env_lock, temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("enable conversation archive for the regression");

        let log_path = temp.path().join("codex.jsonl");
        std::fs::write(&log_path, "").expect("create empty provider log");
        let source_key = "codex:session:offset-upgrade";
        let context = crate::state::conversation_archive::ConversationArchiveContext {
            agent_id: "agent-1".to_string(),
            agent_name: "Synthetic Codex".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["provider-session-1".to_string()],
            provider_source_key: Some(source_key.to_string()),
        };
        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        let initial = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1", "codex", &log_path, source_key, None, true,
        )
        .expect("capture empty source at offset zero");
        assert!(initial.events.is_empty());
        archive
            .append_provider_log_batch_with_context(context.clone(), &[], None, &initial.next)
            .expect("persist the pre-row cursor at offset zero");

        let raw_line = r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Durable response"}]}}"#;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .and_then(|mut file| writeln!(file, "{raw_line}"))
            .expect("append one identity-less provider row");

        let mut pre_upgrade =
            crate::commands::provider_log_acquisition::acquire_provider_log_batch(
                "agent-1",
                "codex",
                &log_path,
                source_key,
                Some(initial.next.clone()),
                true,
            )
            .expect("capture the row as the pre-upgrade writer did");
        assert_eq!(pre_upgrade.events.len(), 1);
        decorate_forward_provider_log_events(&mut pre_upgrade.events, "codex", &log_path);
        let mut legacy_event = pre_upgrade.events[0].clone();
        let legacy_id = legacy_provider_log_event_id(&legacy_event, &log_path);
        let offset_id = legacy_event.id.clone();
        assert_ne!(legacy_id, offset_id);
        legacy_event.id = legacy_id.clone();
        for key in [
            PROVIDER_LOG_ROW_OFFSET_METADATA_KEY,
            "legacy_event_ids",
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
        ] {
            legacy_event.metadata.as_object_mut().unwrap().remove(key);
        }

        archive
            .append_chat_events_with_context(context.clone(), std::slice::from_ref(&legacy_event))
            .expect("persist the old event and narrative before its cursor");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("read cursor left behind by the old writer"),
            Some(initial.next.clone()),
            "the archived observation must remain ahead of the source cursor"
        );

        let mut upgraded_retry =
            crate::commands::provider_log_acquisition::acquire_provider_log_batch(
                "agent-1",
                "codex",
                &log_path,
                source_key,
                Some(initial.next.clone()),
                true,
            )
            .expect("retry the same row after the identity upgrade");
        decorate_forward_provider_log_events(&mut upgraded_retry.events, "codex", &log_path);
        attach_archive_bound_provider_log_retry_aliases(
            &archive,
            &context,
            &mut upgraded_retry.events,
            &log_path,
        )
        .expect("bind the unique historical archive row at the normal caller boundary");
        let retry_event = &upgraded_retry.events[0];
        assert_eq!(retry_event.id, offset_id);
        assert_eq!(
            retry_event.metadata["legacy_event_ids"][0].as_str(),
            Some(legacy_id.as_str()),
            "a complete unique source row carries its pre-offset archive identity"
        );
        assert_eq!(retry_event.source, legacy_event.source);
        assert_eq!(retry_event.turn_id, legacy_event.turn_id);
        assert_eq!(retry_event.role, legacy_event.role);
        assert_eq!(retry_event.text, legacy_event.text);
        assert_eq!(
            retry_event.metadata["raw_type"],
            legacy_event.metadata["raw_type"]
        );

        archive.fail_compatibility_stage_for_test(2);
        let error = archive
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &upgraded_retry.events,
                upgraded_retry.previous.as_ref(),
                &upgraded_retry.next,
                upgraded_retry.source_proof(),
            )
            .expect_err("prepared migration cannot advance the cursor before archive publication");
        assert!(
            error
                .to_string()
                .contains("injected compatibility failure after prepare"),
            "{error}"
        );
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .unwrap(),
            Some(initial.next.clone())
        );
        assert!(archive.chat_events_for_capture(&context).unwrap()[0]
            .metadata
            .get("chat_source_ref")
            .is_none());
        archive.fail_next_chat_cursor_commit_for_test();
        let error = archive
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &upgraded_retry.events,
                upgraded_retry.previous.as_ref(),
                &upgraded_retry.next,
                upgraded_retry.source_proof(),
            )
            .expect_err("archive publication succeeds before the injected cursor failure");
        assert!(
            error
                .to_string()
                .contains("injected chat cursor commit failure"),
            "{error}"
        );
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("cursor remains unchanged after simulated failure"),
            Some(initial.next.clone())
        );

        drop(archive);
        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        let mut replay = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            source_key,
            Some(initial.next.clone()),
            true,
        )
        .expect("replay after archive success and cursor failure");
        decorate_forward_provider_log_events(&mut replay.events, "codex", &log_path);
        attach_archive_bound_provider_log_retry_aliases(
            &archive,
            &context,
            &mut replay.events,
            &log_path,
        )
        .expect("bind the same upgraded owner after cursor publication failure");
        assert_eq!(replay.events[0].id, retry_event.id);
        assert_eq!(replay.events[0].metadata["legacy_event_ids"][0], legacy_id);
        archive
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &replay.events,
                replay.previous.as_ref(),
                &replay.next,
                replay.source_proof(),
            )
            .expect("reconcile the archive and commit the retried cursor");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("read committed cursor"),
            Some(replay.next.clone())
        );

        let archived = archive
            .chat_events_for_capture(&context)
            .expect("read canonical archived events");
        assert_eq!(archived.len(), 1, "the source row has one durable event");
        assert_eq!(
            archived[0].id, legacy_id,
            "historical canonical identity is retained"
        );
        assert_eq!(archived[0].metadata["chat_source_ref"], offset_id);
        assert_eq!(archived[0].metadata["chat_source_start"], 0);
        assert_eq!(
            archived[0].metadata["chat_source_end"],
            (raw_line.len() + 1) as u64
        );
        assert!(archived.iter().any(|event| {
            event.id == legacy_id
                || event.metadata["legacy_event_ids"]
                    .as_array()
                    .is_some_and(|aliases| aliases.iter().any(|alias| alias == &legacy_id))
        }));

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active synthetic conversation");
        let conversation_dir =
            wardian_core::paths::agent_conversation_dir("agent-1", &conversation_id)
                .expect("synthetic conversation directory");
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("conversation.jsonl"),
            )
            .expect("read durable narrative records");
        assert_eq!(records.len(), 1, "the source row has one narrative record");
        assert!(records[0].event_refs.contains(&legacy_id));
        assert_eq!(
            records[0].event_refs.iter().collect::<HashSet<_>>().len(),
            records[0].event_refs.len(),
            "replayed aliases must not duplicate durable references"
        );
    }

    #[tokio::test]
    async fn oversized_identityless_retry_uses_archived_source_sequence() {
        use std::io::Write as _;

        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("isolated Wardian home");
        let _home = WardianHomeGuard::set(&env_lock, temp.path());
        let log_path = temp.path().join("codex.jsonl");
        let filler = format!(r#"{{"type":"ignored","padding":"{}"}}"#, "x".repeat(4096));
        {
            let mut file = std::fs::File::create(&log_path).expect("create oversized source");
            for _ in 0..600 {
                writeln!(file, "{filler}").expect("write ignored source prefix");
            }
        }
        assert!(
            std::fs::metadata(&log_path).unwrap().len() > PROVIDER_LOG_TAIL_BYTES,
            "the fixture must exceed the complete-snapshot limit"
        );

        let source_key = "codex:session:oversized-offset-upgrade";
        let context = ConversationArchiveContext {
            agent_id: "agent-1".to_string(),
            agent_name: "Synthetic Codex".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["provider-session-1".to_string()],
            provider_source_key: Some(source_key.to_string()),
        };
        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        let initial = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1", "codex", &log_path, source_key, None, false,
        )
        .expect("start at the already-existing oversized source prefix");
        assert!(initial.events.is_empty());
        archive
            .append_provider_log_batch_with_context(context.clone(), &[], None, &initial.next)
            .expect("persist the cursor before the duplicate rows");

        let raw_line = r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Repeated durable response"}]}}"#;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .and_then(|mut file| writeln!(file, "{raw_line}\n{raw_line}"))
            .expect("append two identical provider rows");

        let mut retry = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            source_key,
            Some(initial.next.clone()),
            true,
        )
        .expect("acquire both rows from the persisted cursor");
        assert_eq!(retry.events.len(), 2);
        decorate_forward_provider_log_events(&mut retry.events, "codex", &log_path);
        let first_legacy_id = legacy_provider_log_event_id(&retry.events[0], &log_path);
        assert_eq!(
            first_legacy_id,
            legacy_provider_log_event_id(&retry.events[1], &log_path),
            "the duplicate rows share the pre-offset field hash"
        );
        assert_ne!(retry.events[0].id, retry.events[1].id);
        assert!(retry.events.iter().all(|event| {
            event.metadata["legacy_event_ids"]
                .as_array()
                .is_none_or(Vec::is_empty)
        }));

        let mut legacy_event = retry.events[0].clone();
        legacy_event.id = first_legacy_id.clone();
        let metadata = legacy_event
            .metadata
            .as_object_mut()
            .expect("provider metadata object");
        metadata.remove(PROVIDER_LOG_ROW_OFFSET_METADATA_KEY);
        metadata.remove("legacy_event_ids");
        archive
            .append_chat_events_with_context(context.clone(), &[legacy_event.clone()])
            .expect("publish one pre-offset row while its cursor remains behind");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("read cursor before retry")
                .expect("persisted cursor")
                .committed_offset,
            initial.next.committed_offset
        );

        attach_archive_bound_provider_log_retry_aliases(
            &archive,
            &context,
            &mut retry.events,
            &log_path,
        )
        .expect("read archive-bound retry identities");
        assert_eq!(
            retry.events[0].metadata["legacy_event_ids"][0],
            first_legacy_id
        );
        assert!(retry.events[1].metadata["legacy_event_ids"]
            .as_array()
            .is_none_or(Vec::is_empty));
        assert_eq!(retry.events[0].sequence, legacy_event.sequence);
        assert_eq!(retry.events[0].source, legacy_event.source);
        assert_eq!(retry.events[0].turn_id, legacy_event.turn_id);
        assert_eq!(retry.events[0].role, legacy_event.role);
        assert_eq!(retry.events[0].text, legacy_event.text);
        assert_eq!(
            retry.events[0].metadata["raw_type"],
            legacy_event.metadata["raw_type"]
        );

        // Publish the identity upgrade first, then retry the still-uncommitted
        // batch just as capture does after a cursor-write failure.
        archive
            .append_chat_events_with_context(context.clone(), &retry.events)
            .expect("publish upgraded rows before cursor commit");
        let mut replay = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            source_key,
            Some(initial.next.clone()),
            true,
        )
        .expect("replay both rows after archive publication");
        decorate_forward_provider_log_events(&mut replay.events, "codex", &log_path);
        attach_archive_bound_provider_log_retry_aliases(
            &archive,
            &context,
            &mut replay.events,
            &log_path,
        )
        .expect("reconcile retry against the upgraded archive");
        archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &replay.events,
                replay.previous.as_ref(),
                &replay.next,
            )
            .expect("commit the source cursor after idempotent archive replay");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("read cursor after retry")
                .as_ref(),
            Some(&replay.next)
        );

        let archived = archive
            .chat_events_for_capture(&context)
            .expect("read canonical archived rows");
        assert_eq!(archived.len(), 2, "both repeated source rows stay distinct");
        let archived_first = archived
            .iter()
            .find(|event| event.sequence == retry.events[0].sequence)
            .expect("first source row");
        let archived_second = archived
            .iter()
            .find(|event| event.sequence == retry.events[1].sequence)
            .expect("second source row");
        assert_ne!(archived_first.id, archived_second.id);
        assert!(
            archived_first.id == first_legacy_id
                || archived_first.metadata["legacy_event_ids"]
                    .as_array()
                    .is_some_and(|aliases| aliases.iter().any(|alias| alias == &first_legacy_id))
        );
        assert!(archived_second.metadata["legacy_event_ids"]
            .as_array()
            .is_none_or(Vec::is_empty));

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active synthetic conversation");
        let conversation_dir =
            wardian_core::paths::agent_conversation_dir("agent-1", &conversation_id)
                .expect("synthetic conversation directory");
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("conversation.jsonl"),
            )
            .expect("read durable narrative records");
        assert_eq!(records.len(), 2, "both repeated rows retain a narrative");
        assert!(records
            .iter()
            .any(|record| record.event_refs.contains(&first_legacy_id)));
        for record in records {
            assert_eq!(
                record.event_refs.iter().collect::<HashSet<_>>().len(),
                record.event_refs.len(),
                "retry aliases must not duplicate durable references"
            );
        }
    }

    #[tokio::test]
    async fn provider_log_capture_keeps_relationships_before_an_unseen_two_mib_burst() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("save enabled logging setting");
        let log_path = temp.path().join("codex.jsonl");
        let tool_call = r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"shell_command","call_id":"call-leading","input":{"command":"npm test"}}}"#;
        let filler = format!(r#"{{"type":"ignored","padding":"{}"}}"#, "x".repeat(1024));
        let tool_result = r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-leading","output":"tests passed"}}"#;
        let answer = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Finished after the burst"}]}}"#;
        let mut content = String::from(tool_call);
        content.push('\n');
        while content.len() <= PROVIDER_LOG_TAIL_BYTES as usize + filler.len() {
            content.push_str(&filler);
            content.push('\n');
        }
        content.push_str(tool_result);
        content.push('\n');
        content.push_str(answer);
        content.push('\n');
        std::fs::write(&log_path, content).expect("write oversized provider log");

        let state = AppState::new();
        state.agents.lock().await.insert(
            "agent-1".to_string(),
            crate::state::ActiveAgent {
                config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                    session_id: "agent-1".to_string(),
                    session_name: "Agent One".to_string(),
                    agent_class: "Coder".to_string(),
                    provider: "codex".to_string(),
                    folder: temp.path().to_string_lossy().to_string(),
                    fresh_provider_session_id: Some("codex-session-one".to_string()),
                    conversation_logging: AgentConversationLoggingSetting::Default,
                    ..Default::default()
                })),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: None,
                process_id: None,
                query_count: Arc::new(Mutex::new(0)),
                init_timestamp: Arc::new(Mutex::new(None)),
                last_query_timestamp: Arc::new(Mutex::new(None)),
                current_status: Arc::new(Mutex::new("Idle".to_string())),
                last_status_at: Arc::new(Mutex::new(None)),
                watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                    "agent-1".to_string(),
                    32,
                    4096,
                ))),
                terminal_title: Arc::new(Mutex::new(String::new())),
                last_output_at: Arc::new(Mutex::new(None)),
                log_path: Arc::new(Mutex::new(Some(log_path.clone()))),
                log_last_modified: Arc::new(Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );

        archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
            .await
            .expect("drain bounded provider batches");
        let snapshot = agent_archive_capture_snapshot(&state, "agent-1")
            .await
            .expect("capture snapshot");
        let context = conversation_archive_context_from_snapshot(&snapshot);
        let events = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read archived provider events");

        assert!(events.iter().any(|event| {
            event.kind == AgentChatEventKind::ToolCall
                && event.turn_id.as_deref() == Some("call-leading")
        }));
        assert!(events.iter().any(|event| {
            event.kind == AgentChatEventKind::ToolResult
                && event.turn_id.as_deref() == Some("call-leading")
        }));
        assert!(events.iter().any(|event| {
            event.role == Some(AgentChatRole::Assistant)
                && event.text.as_deref() == Some("Finished after the burst")
        }));
        let capture = state
            .conversation_archive
            .provider_log_capture_state("agent-1", "codex:session:codex-session-one")
            .expect("read provider cursor")
            .expect("provider cursor");
        assert_eq!(capture.status, "complete");
        assert_eq!(
            capture.committed_offset,
            std::fs::metadata(&log_path).unwrap().len()
        );
        std::env::remove_var("WARDIAN_HOME");
    }

    #[tokio::test]
    async fn incremental_codex_user_mirror_capture_separates_legacy_turn_and_unbound_lines() {
        use std::io::Write as _;

        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("isolated home");
        let _home_guard = WardianHomeGuard::set(&env_lock, temp.path());

        let log_path = temp.path().join("codex.jsonl");
        let source_key = "codex:session:codex-session-one";
        let prompt = "repeated synthetic request ".repeat(16) + "request";
        let prompt = prompt.chars().take(421).collect::<String>();
        assert_eq!(prompt.len(), 421);

        let turn_lines = |turn_id: &str, message_id: &str| {
            vec![
                serde_json::json!({
                    "type": "turn_context",
                    "payload": {"turn_id": turn_id},
                })
                .to_string(),
                serde_json::json!({
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": message_id,
                        "role": "user",
                        "content": [{"type": "input_text", "text": prompt}],
                        "internal_chat_message_metadata_passthrough": {
                            "turn_id": turn_id,
                            "content_item_kinds": ["user.text"],
                        },
                    },
                })
                .to_string(),
                serde_json::json!({
                    "type": "event_msg",
                    "payload": {"type": "user_message", "message": prompt},
                })
                .to_string(),
            ]
        };
        let first_lines = turn_lines("provider-turn-old", "native-message-old");
        std::fs::write(&log_path, format!("{}\n", first_lines.join("\n")))
            .expect("write initial synthetic Codex turn");

        let context = crate::state::conversation_archive::ConversationArchiveContext {
            agent_id: "agent-1".to_string(),
            agent_name: "Synthetic Codex".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["codex-session-one".to_string()],
            provider_source_key: Some(source_key.to_string()),
        };
        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1", "codex", &log_path, source_key, None, true,
        )
        .expect("capture initial synthetic turn");
        decorate_forward_provider_log_events(&mut first.events, "codex", &log_path);
        let old_mirror = first
            .events
            .iter_mut()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("initial user mirror");
        assert_eq!(old_mirror.metadata["provider_turn_id"], "provider-turn-old");
        let old_root = old_mirror.metadata["request_root_id"]
            .as_str()
            .expect("initial request root")
            .to_string();
        // The legacy hash ignored provenance metadata, so changing this
        // classification recreates its old text/source-derived identity.
        let mut legacy_identity = old_mirror.clone();
        legacy_identity.metadata["input_origin"] = serde_json::json!("provider_internal");
        let legacy_id = archive_identity::stable_provider_log_event_id(&legacy_identity, &log_path);
        old_mirror.id = legacy_id.clone();
        old_mirror
            .metadata
            .as_object_mut()
            .expect("mirror metadata")
            .remove("legacy_event_ids");

        archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &first.events,
                None,
                &first.next,
            )
            .expect("archive the legacy-identity turn and commit its cursor");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("read first cursor"),
            Some(first.next.clone())
        );

        let second_lines = turn_lines("provider-turn-new", "native-message-new");
        let mut append = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("open synthetic Codex log for append");
        writeln!(append, "{}", second_lines.join("\n")).expect("append next synthetic turn");
        drop(append);

        let mut second = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            source_key,
            Some(first.next.clone()),
            true,
        )
        .expect("capture next synthetic turn incrementally");
        assert_eq!(second.events.len(), 2);
        decorate_forward_provider_log_events(&mut second.events, "codex", &log_path);
        let new_mirror = second
            .events
            .iter()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("next user mirror");
        assert_eq!(new_mirror.text.as_deref(), Some(prompt.as_str()));
        assert_eq!(new_mirror.text.as_deref().unwrap().len(), 421);
        assert_eq!(new_mirror.turn_id, None);
        assert_eq!(new_mirror.created_at, None);
        assert_eq!(new_mirror.metadata["provider_turn_id"], "provider-turn-new");
        assert_ne!(new_mirror.metadata["request_root_id"], old_root);
        assert_ne!(new_mirror.id, legacy_id);
        assert!(new_mirror.metadata.get("legacy_event_ids").is_none());
        let new_mirror_id = new_mirror.id.clone();
        let new_root = new_mirror.metadata["request_root_id"]
            .as_str()
            .expect("next request root")
            .to_string();

        let mut same_turn_retry =
            crate::commands::provider_log_acquisition::acquire_provider_log_batch(
                "agent-1",
                "codex",
                &log_path,
                source_key,
                Some(first.next.clone()),
                true,
            )
            .expect("retry the same incremental turn from its prior cursor");
        decorate_forward_provider_log_events(&mut same_turn_retry.events, "codex", &log_path);
        assert_eq!(
            same_turn_retry
                .events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            second
                .events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            "retrying the same native turn must reproduce its event identities"
        );
        assert_eq!(same_turn_retry.next, second.next);

        archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &second.events,
                second.previous.as_ref(),
                &second.next,
            )
            .expect("append the distinct turn without claiming the old narrative");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", source_key)
                .expect("read advanced cursor"),
            Some(second.next.clone())
        );
        assert_eq!(
            archive
                .append_provider_log_batch_with_context(
                    context.clone(),
                    &same_turn_retry.events,
                    same_turn_retry.previous.as_ref(),
                    &same_turn_retry.next,
                )
                .expect_err("a stale replay cannot append after its cursor already committed")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );

        let malformed_turn_lines = [
            serde_json::json!({"type": "turn_context"}).to_string(),
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "id": "native-message-unbound",
                    "role": "user",
                    "content": [{"type": "input_text", "text": prompt}],
                    "internal_chat_message_metadata_passthrough": {
                        "content_item_kinds": ["user.text"],
                    },
                },
            })
            .to_string(),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": prompt},
            })
            .to_string(),
        ];
        let mut append = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("reopen synthetic Codex log");
        writeln!(append, "{}", malformed_turn_lines.join("\n"))
            .expect("append unbound synthetic turn");
        drop(append);

        let mut unbound = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            source_key,
            Some(second.next.clone()),
            true,
        )
        .expect("capture malformed turn with no native binding");
        assert_eq!(unbound.events.len(), 2);
        let unbound_mirror = unbound
            .events
            .iter()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("unbound user mirror");
        assert!(unbound_mirror.metadata.get("provider_turn_id").is_none());
        assert!(unbound_mirror.sequence.is_some());
        decorate_forward_provider_log_events(&mut unbound.events, "codex", &log_path);
        let unbound_mirror = unbound
            .events
            .iter()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("decorated unbound mirror");
        let unbound_mirror_id = unbound_mirror.id.clone();
        assert_ne!(unbound_mirror.id, legacy_id);
        assert!(unbound_mirror.metadata.get("legacy_event_ids").is_none());

        let mut unbound_retry =
            crate::commands::provider_log_acquisition::acquire_provider_log_batch(
                "agent-1",
                "codex",
                &log_path,
                source_key,
                Some(second.next.clone()),
                true,
            )
            .expect("retry the unbound log segment");
        decorate_forward_provider_log_events(&mut unbound_retry.events, "codex", &log_path);
        assert_eq!(
            unbound_retry
                .events
                .iter()
                .find(|event| event.source.as_deref() == Some("event_msg"))
                .map(|event| event.id.as_str()),
            Some(unbound_mirror_id.as_str()),
            "the persisted log sequence must keep an unbound mirror stable on retry"
        );
        assert_eq!(unbound_retry.next, unbound.next);

        archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &unbound.events,
                unbound.previous.as_ref(),
                &unbound.next,
            )
            .expect("append unbound request using its log sequence identity");

        let retry = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            source_key,
            Some(unbound.next.clone()),
            true,
        )
        .expect("retry capture at committed end of log");
        assert!(retry.events.is_empty());
        archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &retry.events,
                retry.previous.as_ref(),
                &retry.next,
            )
            .expect("retry empty capture without duplicating archive history");
        assert_eq!(retry.next, unbound.next);

        let archived = archive
            .chat_events_for_capture(&context)
            .expect("read archived synthetic transcript");
        assert_eq!(
            archived
                .iter()
                .filter(|event| {
                    event.source.as_deref() == Some("event_msg")
                        && event.text.as_deref() == Some(prompt.as_str())
                })
                .filter_map(|event| event.metadata["provider_turn_id"].as_str())
                .collect::<Vec<_>>(),
            vec!["provider-turn-old", "provider-turn-new"]
        );
        let archived_mirrors = archived
            .iter()
            .filter(|event| event.source.as_deref() == Some("event_msg"))
            .collect::<Vec<_>>();
        assert_eq!(archived_mirrors.len(), 3);
        assert!(archived_mirrors.iter().all(|event| {
            event.provider == "codex"
                && event.session_id == "agent-1"
                && event.metadata["provider_log"] == true
                && event.metadata["log_path"] == log_path.to_string_lossy().as_ref()
        }));
        let mirror_ids = archived_mirrors
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        assert!(mirror_ids.contains(&legacy_id));
        assert_ne!(mirror_ids[0], mirror_ids[1]);
        assert_ne!(mirror_ids[1], mirror_ids[2]);
        assert_ne!(mirror_ids[0], mirror_ids[2]);

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active synthetic conversation");
        let conversation_dir =
            wardian_core::paths::agent_conversation_dir("agent-1", &conversation_id)
                .expect("synthetic conversation directory");
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(conversation_dir.join("manifest.json"))
                .expect("read synthetic archive manifest"),
        )
        .expect("parse synthetic archive manifest");
        assert_eq!(manifest["provider_source_key"], source_key);
        assert_eq!(
            manifest["provider_session_ids"],
            serde_json::json!(["codex-session-one"])
        );
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("conversation.jsonl"),
            )
            .expect("read synthetic narrative records");
        let legacy_record = records
            .iter()
            .find(|record| record.event_refs.contains(&legacy_id))
            .expect("legacy mirror narrative owner");
        let new_record = records
            .iter()
            .find(|record| record.event_refs.contains(&new_mirror_id))
            .expect("new mirror has its own narrative owner");
        assert_eq!(
            legacy_record.request_root_id.as_deref(),
            Some(old_root.as_str())
        );
        assert_eq!(
            new_record.request_root_id.as_deref(),
            Some(new_root.as_str())
        );
        assert_ne!(legacy_record.seq, new_record.seq);

        let replayed = crate::state::conversation_archive::provenance::merge_current_capture(
            Vec::new(),
            archived,
        )
        .expect("replay archive without text-only deduplication");
        let transcript = merge_chat_events(Vec::new(), replayed);
        let repeated_requests = transcript
            .iter()
            .filter(|event| {
                event.role == Some(AgentChatRole::User)
                    && event.text.as_deref() == Some(prompt.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(repeated_requests.len(), 4);
        assert_eq!(
            repeated_requests
                .iter()
                .filter_map(|event| event.metadata["provider_turn_id"].as_str())
                .collect::<Vec<_>>(),
            vec!["provider-turn-old", "provider-turn-new"]
        );
    }

    #[tokio::test]
    async fn claude_raw_line_identity_recovers_capture_without_rewriting_legacy_alias_history() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp dir");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("enable conversation logging");

        let log_path = temp.path().join("claude.jsonl");
        let first_line = r#"{"type":"user","uuid":"request-1","message":{"role":"user","content":"Initial request"}}"#;
        let second_line = r#"{"type":"user","uuid":"request-2","message":{"role":"user","content":"Request blocked by ambiguous history"}}"#;
        std::fs::write(&log_path, format!("{first_line}\n")).expect("write initial Claude log");

        let state = AppState::new();
        state.agents.lock().await.insert(
            "agent-1".to_string(),
            crate::state::ActiveAgent {
                config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                    session_id: "agent-1".to_string(),
                    session_name: "Claude One".to_string(),
                    agent_class: "Coder".to_string(),
                    provider: "claude".to_string(),
                    folder: temp.path().to_string_lossy().to_string(),
                    fresh_provider_session_id: Some("claude-session-one".to_string()),
                    conversation_logging: AgentConversationLoggingSetting::Default,
                    ..Default::default()
                })),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: None,
                process_id: None,
                query_count: Arc::new(Mutex::new(0)),
                init_timestamp: Arc::new(Mutex::new(None)),
                last_query_timestamp: Arc::new(Mutex::new(None)),
                current_status: Arc::new(Mutex::new("Idle".to_string())),
                last_status_at: Arc::new(Mutex::new(None)),
                watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                    "agent-1".to_string(),
                    32,
                    4096,
                ))),
                terminal_title: Arc::new(Mutex::new(String::new())),
                last_output_at: Arc::new(Mutex::new(None)),
                log_path: Arc::new(Mutex::new(Some(log_path.clone()))),
                log_last_modified: Arc::new(Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );

        // This is the same helper called by the best-effort status sync and
        // directly by clear_agent_session_inner before it resets identity.
        archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
            .await
            .expect("capture the first Claude event");
        let source_key = "claude:session:claude-session-one";
        let cursor_before = state
            .conversation_archive
            .provider_log_capture_state("agent-1", source_key)
            .expect("read initial cursor")
            .expect("initial Claude cursor");
        assert_eq!(cursor_before.committed_offset, first_line.len() as u64 + 1);

        use std::io::Write as _;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .expect("open Claude log for append"),
            "{second_line}"
        )
        .expect("append second Claude event");
        let current_event =
            load_provider_log_chat_events("agent-1", "claude", Some(&log_path), &[])
                .into_iter()
                .find(|event| event.text.as_deref() == Some("Request blocked by ambiguous history"))
                .expect("second Claude provider event");

        let snapshot = agent_archive_capture_snapshot(&state, "agent-1")
            .await
            .expect("capture Claude snapshot");
        let context = conversation_archive_context_from_snapshot(&snapshot);
        state
            .conversation_archive
            .append_chat_events_with_context(context.clone(), std::slice::from_ref(&current_event))
            .expect("seed the durable event and its exact raw-line owner");

        let conversation_id = state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation");
        let conversation_dir =
            wardian_core::paths::agent_conversation_dir("agent-1", &conversation_id)
                .expect("conversation directory");
        let records_path = conversation_dir.join("conversation.jsonl");
        let mut records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(&records_path)
                .expect("read initial narrative");
        let current_owner = records
            .iter_mut()
            .find(|record| record.event_refs.contains(&current_event.id))
            .expect("exact raw-line owner");
        current_owner.seq = 594;
        current_owner.at = "2026-09-08T12:00:00Z".to_string();
        current_owner.source_refs = vec!["src_594".to_string()];
        let mut legacy_owner = current_owner.clone();
        let legacy_id = current_event.metadata["legacy_event_ids"][0]
            .as_str()
            .expect("legacy Claude identity")
            .to_string();
        legacy_owner.seq = 780;
        legacy_owner.at = "2026-09-10T12:00:00Z".to_string();
        legacy_owner.event_refs = vec![legacy_id.clone()];
        legacy_owner.source_refs = vec!["src_780".to_string()];
        records.push(legacy_owner.clone());
        wardian_core::conversations::write_jsonl_atomic(&records_path, &records)
            .expect("seed a prior legacy-alias owner in the isolated archive");
        let events_path = conversation_dir.join("events.jsonl");
        let events_before = std::fs::read(&events_path).expect("read durable events");
        let records_before = std::fs::read(&records_path).expect("read durable narratives");

        for path in ["status sync", "clear preflight"] {
            archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
                .await
                .unwrap_or_else(|error| {
                    panic!("{path} should resolve the exact raw-line owner: {error}")
                });
            assert_eq!(
                state
                    .conversation_archive
                    .provider_log_capture_state("agent-1", source_key)
                    .expect("read cursor after retry")
                    .expect("cursor remains present")
                    .committed_offset,
                std::fs::metadata(&log_path)
                    .expect("Claude log metadata")
                    .len(),
                "{path} must commit the provider cursor after a successful exact match"
            );
            assert_eq!(
                std::fs::read(&events_path).expect("read durable events after retry"),
                events_before,
                "{path} must not duplicate the already durable raw event"
            );
            assert_eq!(
                std::fs::read(&records_path).expect("read narratives after retry"),
                records_before,
                "{path} must preserve both distinct narrative owners and source positions"
            );
        }

        assert!(cursor_before.committed_offset < std::fs::metadata(&log_path).unwrap().len());
        assert!(records.len() > 1);
        assert_eq!(records.last(), Some(&legacy_owner));

        match previous_home {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[tokio::test]
    async fn incremental_opencode_capture_persists_bound_db_events_once_and_respects_disabled_logging(
    ) {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp dir");
        let provider_data = temp.path().join("provider-data");
        let opencode_dir = provider_data.join("opencode");
        std::fs::create_dir_all(&opencode_dir).expect("create OpenCode data dir");
        let db_path = opencode_dir.join("opencode.db");
        let log_path = temp.path().join("opencode.log");
        std::fs::write(&log_path, "").expect("write empty rolling log");

        let previous_wardian_home = std::env::var_os("WARDIAN_HOME");
        let previous_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());
        std::env::set_var("XDG_DATA_HOME", &provider_data);
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("save enabled logging setting");

        let initial_created_at = chrono::Utc::now().timestamp_millis();
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
            "#,
        )
        .expect("seed db");
        conn.execute(
            "INSERT INTO message VALUES ('msg-initial', 'ses_incremental', ?1, ?1, '{\"role\":\"assistant\"}')",
            rusqlite::params![initial_created_at],
        )
        .expect("insert initial message");
        conn.execute(
            "INSERT INTO part VALUES ('part-initial', 'msg-initial', 'ses_incremental', ?1, ?1, '{\"type\":\"text\",\"text\":\"Canonical initial answer\"}')",
            rusqlite::params![initial_created_at + 1],
        )
        .expect("insert initial part");
        drop(conn);

        let watch_state = Arc::new(Mutex::new(AgentWatchState::new(
            "agent-1".to_string(),
            32,
            4096,
        )));
        watch_state
            .lock()
            .expect("lock watch state")
            .push_transcript(WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "Canonical initial answer".to_string(),
                provider: "opencode".to_string(),
                turn_id: Some("ses_incremental".to_string()),
                source: Some("opencode_db".to_string()),
                provider_provenance: None,
            });

        let state = AppState::new();
        state.agents.lock().await.insert(
            "agent-1".to_string(),
            crate::state::ActiveAgent {
                config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                    session_id: "agent-1".to_string(),
                    session_name: "Agent One".to_string(),
                    agent_class: "Coder".to_string(),
                    provider: "opencode".to_string(),
                    folder: temp.path().to_string_lossy().to_string(),
                    fresh_provider_session_id: Some("ses_incremental".to_string()),
                    conversation_logging: AgentConversationLoggingSetting::Default,
                    ..Default::default()
                })),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: None,
                process_id: None,
                query_count: Arc::new(Mutex::new(0)),
                init_timestamp: Arc::new(Mutex::new(None)),
                last_query_timestamp: Arc::new(Mutex::new(None)),
                current_status: Arc::new(Mutex::new("Idle".to_string())),
                last_status_at: Arc::new(Mutex::new(None)),
                watch_state,
                terminal_title: Arc::new(Mutex::new(String::new())),
                last_output_at: Arc::new(Mutex::new(None)),
                log_path: Arc::new(Mutex::new(Some(log_path))),
                log_last_modified: Arc::new(Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );

        archive_agent_chat_events_for_state(&state, "agent-1")
            .await
            .expect("capture initial OpenCode events");
        let snapshot = agent_archive_capture_snapshot(&state, "agent-1")
            .await
            .expect("capture snapshot");
        let context = conversation_archive_context_from_snapshot(&snapshot);
        let events = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read initial archive");
        let initial = events
            .iter()
            .filter(|event| event.metadata["part_id"] == "part-initial")
            .collect::<Vec<_>>();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].source.as_deref(), Some("opencode_db"));
        assert_eq!(initial[0].metadata["provider_log"], true);
        assert_eq!(
            initial[0].metadata["opencode_session_id"],
            "ses_incremental"
        );
        assert_eq!(
            initial[0].metadata["source_path"],
            db_path.to_string_lossy().as_ref()
        );

        archive_agent_chat_events_for_state(&state, "agent-1")
            .await
            .expect("poll OpenCode events again");
        let repeated = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read repeated archive");
        assert_eq!(
            repeated
                .iter()
                .filter(|event| event.metadata["part_id"] == "part-initial")
                .count(),
            1
        );

        // This answer is written while logging is enabled but is deliberately
        // left as backlog until after the policy transition.
        let backlog_created_at = chrono::Utc::now().timestamp_millis();
        let conn = rusqlite::Connection::open(&db_path).expect("reopen db");
        conn.execute(
            "INSERT INTO message VALUES ('msg-backlog', 'ses_incremental', ?1, ?1, '{\"role\":\"assistant\"}')",
            rusqlite::params![backlog_created_at],
        )
        .expect("append enabled backlog message");
        conn.execute(
            "INSERT INTO part VALUES ('part-backlog', 'msg-backlog', 'ses_incremental', ?1, ?1, '{\"type\":\"text\",\"text\":\"Enabled backlog answer\"}')",
            rusqlite::params![backlog_created_at + 1],
        )
        .expect("append enabled backlog part");
        drop(conn);

        // Exercise the real #1253 policy transition. No archive/transcript
        // poll occurs while logging is disabled.
        crate::commands::settings::save_shell_settings_for_state(
            &state,
            crate::utils::ShellSettingsDocument {
                schema_version: 2,
                settings: crate::utils::ShellSettings {
                    conversation_logging: ConversationLoggingSetting::Disabled,
                    ..Default::default()
                },
                overrides: crate::utils::ShellSettingsOverrides {
                    conversation_logging: Some(ConversationLoggingSetting::Disabled),
                    ..Default::default()
                },
            },
        )
        .await
        .expect("disable logging through policy transition");

        std::thread::sleep(std::time::Duration::from_millis(2));
        let disabled_created_at = chrono::Utc::now().timestamp_millis();
        let conn = rusqlite::Connection::open(&db_path).expect("reopen db while disabled");
        conn.execute(
            "INSERT INTO message VALUES ('msg-disabled', 'ses_incremental', ?1, ?1, '{\"role\":\"assistant\"}')",
            rusqlite::params![disabled_created_at],
        )
        .expect("append disabled answer message");
        conn.execute(
            "INSERT INTO part VALUES ('part-disabled', 'msg-disabled', 'ses_incremental', ?1, ?1, '{\"type\":\"text\",\"text\":\"Must remain live-only\"}')",
            rusqlite::params![disabled_created_at],
        )
        .expect("append disabled answer part");
        drop(conn);

        std::thread::sleep(std::time::Duration::from_millis(2));
        crate::commands::settings::save_shell_settings_for_state(
            &state,
            crate::utils::ShellSettingsDocument {
                schema_version: 2,
                settings: crate::utils::ShellSettings {
                    conversation_logging: ConversationLoggingSetting::Enabled,
                    ..Default::default()
                },
                overrides: crate::utils::ShellSettingsOverrides::default(),
            },
        )
        .await
        .expect("re-enable logging through policy transition");

        archive_agent_chat_events_for_state(&state, "agent-1")
            .await
            .expect("capture after no-poll disabled window");
        let after_reenable = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read archive after re-enable");
        assert!(after_reenable
            .iter()
            .all(|event| event.metadata["part_id"] != "part-disabled"));
        assert_eq!(
            after_reenable
                .iter()
                .filter(|event| event.metadata["part_id"] == "part-initial")
                .count(),
            1
        );
        assert_eq!(
            after_reenable
                .iter()
                .filter(|event| event.metadata["part_id"] == "part-backlog")
                .count(),
            1
        );

        // A disabled poll must not move the original interval start past a
        // canonical row created earlier in that same disabled window.
        crate::commands::settings::save_shell_settings_for_state(
            &state,
            crate::utils::ShellSettingsDocument {
                schema_version: 2,
                settings: crate::utils::ShellSettings {
                    conversation_logging: ConversationLoggingSetting::Disabled,
                    ..Default::default()
                },
                overrides: crate::utils::ShellSettingsOverrides {
                    conversation_logging: Some(ConversationLoggingSetting::Disabled),
                    ..Default::default()
                },
            },
        )
        .await
        .expect("disable logging for polled window");

        std::thread::sleep(std::time::Duration::from_millis(2));
        let polled_disabled_created_at = chrono::Utc::now().timestamp_millis();
        let conn = rusqlite::Connection::open(&db_path).expect("reopen db for disabled poll");
        conn.execute(
            "INSERT INTO message VALUES ('msg-disabled-polled', 'ses_incremental', ?1, ?1, '{\"role\":\"assistant\"}')",
            rusqlite::params![polled_disabled_created_at],
        )
        .expect("append polled disabled answer message");
        conn.execute(
            "INSERT INTO part VALUES ('part-disabled-polled', 'msg-disabled-polled', 'ses_incremental', ?1, ?1, '{\"type\":\"text\",\"text\":\"Must remain live-only after poll\"}')",
            rusqlite::params![polled_disabled_created_at],
        )
        .expect("append polled disabled answer part");
        drop(conn);

        archive_agent_chat_events_for_state(&state, "agent-1")
            .await
            .expect("capture disabled poll");
        std::thread::sleep(std::time::Duration::from_millis(2));
        crate::commands::settings::save_shell_settings_for_state(
            &state,
            crate::utils::ShellSettingsDocument {
                schema_version: 2,
                settings: crate::utils::ShellSettings {
                    conversation_logging: ConversationLoggingSetting::Enabled,
                    ..Default::default()
                },
                overrides: crate::utils::ShellSettingsOverrides::default(),
            },
        )
        .await
        .expect("re-enable logging after disabled poll");
        archive_agent_chat_events_for_state(&state, "agent-1")
            .await
            .expect("capture after polled disabled window");
        let after_polled_reenable = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read archive after polled re-enable");
        assert!(after_polled_reenable
            .iter()
            .all(|event| event.metadata["part_id"] != "part-disabled-polled"));

        match previous_wardian_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_xdg_data_home {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    #[tokio::test]
    async fn initially_disabled_fresh_provider_prefix_is_not_archived() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Disabled,
            ..Default::default()
        })
        .expect("save disabled logging setting");
        let log_path = temp.path().join("codex.jsonl");
        std::fs::write(
            &log_path,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"SECRET_BEFORE_FIRST_CAPTURE\"}}\n",
        )
        .expect("write pre-existing provider event");
        let existing_len = std::fs::metadata(&log_path)
            .expect("provider metadata")
            .len();

        let state = AppState::new();
        state.agents.lock().await.insert(
            "agent-1".to_string(),
            crate::state::ActiveAgent {
                config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                    session_id: "agent-1".to_string(),
                    session_name: "Agent One".to_string(),
                    agent_class: "Coder".to_string(),
                    provider: "codex".to_string(),
                    folder: temp.path().to_string_lossy().to_string(),
                    fresh_provider_session_id: Some("codex-session-one".to_string()),
                    conversation_logging: AgentConversationLoggingSetting::Default,
                    ..Default::default()
                })),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: None,
                process_id: None,
                query_count: Arc::new(Mutex::new(0)),
                init_timestamp: Arc::new(Mutex::new(None)),
                last_query_timestamp: Arc::new(Mutex::new(None)),
                current_status: Arc::new(Mutex::new("Idle".to_string())),
                last_status_at: Arc::new(Mutex::new(None)),
                watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                    "agent-1".to_string(),
                    32,
                    4096,
                ))),
                terminal_title: Arc::new(Mutex::new(String::new())),
                last_output_at: Arc::new(Mutex::new(None)),
                log_path: Arc::new(Mutex::new(Some(log_path))),
                log_last_modified: Arc::new(Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );

        archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
            .await
            .expect("observe initially disabled source");
        let snapshot = agent_archive_capture_snapshot(&state, "agent-1")
            .await
            .expect("capture snapshot");
        let context = conversation_archive_context_from_snapshot(&snapshot);
        let events = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read archived provider events");
        assert!(events
            .iter()
            .all(|event| event.text.as_deref() != Some("SECRET_BEFORE_FIRST_CAPTURE")));
        let capture = state
            .conversation_archive
            .provider_log_capture_state("agent-1", "codex:session:codex-session-one")
            .expect("read provider cursor")
            .expect("provider cursor");
        assert_eq!(capture.committed_offset, existing_len);
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn lifecycle_boundary_does_not_reimport_append_only_log_tail() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        std::fs::write(
            &log_path,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Must come through the cursor\"}}\n",
        )
        .expect("write provider log");
        let snapshot = AgentArchiveCaptureSnapshot {
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            resume_session: Some("provider-session-1".to_string()),
            fresh_provider_session_id: None,
            cleared_provider_sessions: Vec::new(),
            current_status: "Idle".to_string(),
            last_status_at: None,
            log_path: Some(log_path),
            agent_name: "Agent One".to_string(),
            agent_class: "Coder".to_string(),
            workspace: temp.path().to_string_lossy().to_string(),
            agent_conversation_logging: AgentConversationLoggingSetting::Default,
            watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                "agent-1".to_string(),
                32,
                4096,
            ))),
        };

        let capture = collect_agent_chat_events_for_boundary(&snapshot)
            .expect("collect lifecycle boundary events");
        assert!(capture
            .events
            .iter()
            .all(|event| event.text.as_deref() != Some("Must come through the cursor")));
        assert!(capture
            .events
            .iter()
            .all(|event| event.metadata["provider_log"] != true));
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

        let (content, _) =
            read_provider_log_tail_with_limit_and_offset(&log_path, 256).expect("tail content");

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
        let (content, _) = read_provider_log_tail_with_limit_and_offset(&log_path, tail_bytes)
            .expect("tail content");

        assert!(!content.contains(stale_line));
        assert!(content.contains("Boundary provider log message"));
        assert!(content.starts_with('{'));
    }

    #[test]
    fn provider_log_tail_keeps_lossy_utf8_and_tracks_raw_row_offsets() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("pi.jsonl");
        let large_prefix = format!(
            r#"{{"type":"ignored","padding":"{}"}}"#,
            "x".repeat(PROVIDER_LOG_TAIL_BYTES as usize + 256)
        );
        let mut bytes = large_prefix.into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(br#"{"type":"ignored","padding":"bad"#);
        bytes.push(0xff);
        bytes.extend_from_slice(b"\"}\n");
        let target_offset = bytes.len() as u64;
        let target =
            br#"{"type":"message","message":{"role":"user","content":"After invalid UTF-8"}}"#;
        bytes.extend_from_slice(target);
        bytes.push(b'\n');
        std::fs::write(&log_path, bytes).expect("write provider log with invalid UTF-8");

        let events = load_provider_log_chat_events("agent-1", "pi", Some(&log_path), &[]);
        let event = events
            .iter()
            .find(|event| event.text.as_deref() == Some("After invalid UTF-8"))
            .expect("valid row after lossy tail decoding");
        assert_eq!(
            event.metadata["provider_log_row_offset"].as_u64(),
            Some(target_offset)
        );
    }

    #[test]
    fn fallback_provider_log_ids_follow_absolute_row_offsets() {
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

        assert_ne!(first_id, second_id);
    }

    #[test]
    fn codex_rows_in_the_same_provider_turn_keep_distinct_fallback_ids() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        let line = r#"{"type":"response_item","turn_id":"turn-shared","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Repeated response"}]}}"#;
        std::fs::write(&log_path, format!("{line}\n{line}\n")).expect("write duplicate rows");

        let events = load_provider_log_chat_events("agent-1", "codex", Some(&log_path), &[]);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].turn_id, events[1].turn_id);
        assert_ne!(events[0].id, events[1].id);
        assert_eq!(
            events[0].metadata["provider_log_row_offset"].as_u64(),
            Some(0)
        );
        assert_eq!(
            events[1].metadata["provider_log_row_offset"].as_u64(),
            Some((line.len() + 1) as u64)
        );
        assert!(events
            .iter()
            .all(|event| event.metadata.get("legacy_event_ids").is_none()));
    }

    #[tokio::test]
    async fn legacy_pending_codex_state_recovers_forward_tail_identity() {
        use std::io::Write as _;

        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp dir");
        let _home = WardianHomeGuard::set(&env_lock, temp.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("enable archive for legacy pending-state replay");
        let log_path = temp.path().join("codex.jsonl");
        let context = r#"{"type":"response_item","payload":{"type":"message","id":"context-1","role":"user","content":[{"type":"input_text","text":"Host context."}],"internal_chat_message_metadata_passthrough":{"turn_id":"codex-turn-1"}}}"#;
        std::fs::write(&log_path, format!("{context}\n")).expect("write pending context");

        let first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            "codex:session:one",
            None,
            true,
        )
        .expect("capture context into pending normalization state");
        assert!(first.events.is_empty());
        assert!(first.next.normalizer.has_pending_events());

        let mut old_state = serde_json::to_value(&first.next).expect("serialize current state");
        let old_offset = old_state["normalizer"]["pending_events"][0]["metadata"]
            .as_object_mut()
            .expect("pending event metadata")
            .remove("provider_log_row_offset")
            .expect("new state has an absolute row offset");
        assert_eq!(old_offset.as_u64(), Some(0));
        for key in [
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
        ] {
            old_state["normalizer"]["pending_events"][0]["metadata"]
                .as_object_mut()
                .unwrap()
                .remove(key);
        }
        let old_state: crate::commands::provider_log_acquisition::ProviderLogCaptureState =
            serde_json::from_value(old_state).expect("restore pre-offset capture state");

        let request = r#"{"type":"event_msg","payload":{"type":"user_message","message":"Inspect the archive."}}"#;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .and_then(|mut file| writeln!(file, "{request}"))
            .expect("append the request that releases pending context");
        let mut next = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            "codex:session:one",
            Some(old_state.clone()),
            true,
        )
        .expect("continue from pre-offset state");
        let forward_context = next
            .events
            .iter()
            .find(|event| event.metadata["input_origin"] == "context_injection")
            .expect("released pending context");
        assert_eq!(forward_context.sequence, Some(1));
        assert_eq!(
            forward_context.metadata["provider_log_row_offset"].as_u64(),
            Some(0)
        );
        let expected_source_ref = super::super::chat_recent_seed::source_reference(
            "agent-1",
            &old_state.native_identity,
            0,
            context,
            0,
        );
        assert_eq!(
            forward_context.metadata["chat_source_ref"],
            expected_source_ref
        );
        assert_eq!(forward_context.metadata["chat_source_start"], 0);
        assert_eq!(
            forward_context.metadata["chat_source_end"],
            (context.len() + 1) as u64
        );
        decorate_forward_provider_log_events(&mut next.events, "codex", &log_path);
        let forward_id = next
            .events
            .iter()
            .find(|event| event.metadata["input_origin"] == "context_injection")
            .expect("decorated forward context")
            .id
            .clone();

        let mut replay = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            "codex:session:one",
            Some(old_state.clone()),
            true,
        )
        .expect("replay the same source range from the same legacy state");
        decorate_forward_provider_log_events(&mut replay.events, "codex", &log_path);
        let replayed_context_id = replay
            .events
            .iter()
            .find(|event| event.metadata["input_origin"] == "context_injection")
            .expect("replayed legacy context")
            .id
            .clone();
        assert_eq!(forward_id, replayed_context_id);

        let full_tail = load_provider_log_chat_events("agent-1", "codex", Some(&log_path), &[]);
        let projected_context = full_tail
            .iter()
            .find(|event| event.metadata["input_origin"] == "context_injection")
            .expect("full-tail context projection");
        let mut framed_snapshot = projected_context.clone();
        framed_snapshot.metadata["chat_source_ref"] =
            serde_json::json!(super::super::chat_recent_seed::source_reference(
                "agent-1",
                &old_state.native_identity,
                0,
                context,
                0,
            ));
        decorate_forward_provider_log_events(
            std::slice::from_mut(&mut framed_snapshot),
            "codex",
            &log_path,
        );
        assert_eq!(forward_id, framed_snapshot.id);
        assert_eq!(forward_id, expected_source_ref);

        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        let archive_context = crate::state::conversation_archive::ConversationArchiveContext {
            agent_id: "agent-1".to_string(),
            agent_name: "Synthetic Codex".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["provider-session-1".to_string()],
            provider_source_key: Some("codex:session:one".to_string()),
        };
        archive
            .append_provider_log_batch_with_context(archive_context.clone(), &[], None, &old_state)
            .expect("persist the historical pending cursor");
        let mut legacy_context = projected_context.clone();
        let legacy_id = legacy_provider_log_event_id(&legacy_context, &log_path);
        legacy_context.id = legacy_id.clone();
        for key in [
            PROVIDER_LOG_ROW_OFFSET_METADATA_KEY,
            "legacy_event_ids",
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
        ] {
            legacy_context.metadata.as_object_mut().unwrap().remove(key);
        }
        archive
            .append_chat_events_with_context(archive_context.clone(), &[legacy_context])
            .expect("persist the true pre-coordinate context observation");
        attach_archive_bound_provider_log_retry_aliases(
            &archive,
            &archive_context,
            &mut next.events,
            &log_path,
        )
        .expect("bind the qualified historical context row");
        let context_event = next
            .events
            .iter()
            .find(|event| event.metadata["input_origin"] == "context_injection")
            .unwrap();
        assert_eq!(context_event.metadata["legacy_event_ids"][0], legacy_id);
        archive.fail_next_chat_cursor_commit_for_test();
        let error = archive
            .append_verified_provider_log_batch_with_context(
                archive_context.clone(),
                &next.events,
                next.previous.as_ref(),
                &next.next,
                next.source_proof(),
            )
            .expect_err("publish the framed observations before the injected cursor failure");
        assert!(
            error
                .to_string()
                .contains("injected chat cursor commit failure"),
            "{error}"
        );
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", "codex:session:one")
                .unwrap(),
            Some(old_state)
        );
        drop(archive);
        let archive = crate::state::conversation_archive::ConversationArchiveState::default();
        attach_archive_bound_provider_log_retry_aliases(
            &archive,
            &archive_context,
            &mut replay.events,
            &log_path,
        )
        .expect("bind the same owner on replay");
        archive
            .append_verified_provider_log_batch_with_context(
                archive_context.clone(),
                &replay.events,
                replay.previous.as_ref(),
                &replay.next,
                replay.source_proof(),
            )
            .expect("commit the retry without duplicating the pending observation");
        assert_eq!(
            archive
                .provider_log_capture_state("agent-1", "codex:session:one")
                .unwrap(),
            Some(replay.next)
        );
        let archived = archive.chat_events_for_capture(&archive_context).unwrap();
        let contexts = archived
            .iter()
            .filter(|event| event.metadata["input_origin"] == "context_injection")
            .collect::<Vec<_>>();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].id, legacy_id);
        assert_eq!(contexts[0].metadata["chat_source_ref"], forward_id);
        assert!(contexts[0].metadata["legacy_event_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &forward_id));
    }

    #[test]
    fn legacy_pending_codex_state_without_matching_source_row_fails_closed() {
        use std::io::Write as _;

        let temp = tempfile::tempdir().expect("temp dir");
        let log_path = temp.path().join("codex.jsonl");
        let context = r#"{"type":"response_item","payload":{"type":"message","id":"context-1","role":"user","content":[{"type":"input_text","text":"Host context."}],"internal_chat_message_metadata_passthrough":{"turn_id":"codex-turn-1"}}}"#;
        std::fs::write(&log_path, format!("{context}\n")).expect("write pending context");
        let first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            "codex:session:one",
            None,
            true,
        )
        .expect("capture context into pending normalization state");

        let mut old_state = serde_json::to_value(&first.next).expect("serialize current state");
        let pending_metadata = old_state["normalizer"]["pending_events"][0]["metadata"]
            .as_object_mut()
            .expect("pending event metadata");
        pending_metadata.remove("provider_log_row_offset");
        pending_metadata.remove("_wardian_provider_raw_line");
        old_state["normalizer"]["pending_events"][0]["text"] =
            serde_json::json!("A stale pending context that is absent from the source row.");
        let old_state: crate::commands::provider_log_acquisition::ProviderLogCaptureState =
            serde_json::from_value(old_state).expect("restore state without source row proof");
        let original_offset = old_state.committed_offset;

        let request = r#"{"type":"event_msg","payload":{"type":"user_message","message":"Inspect the archive."}}"#;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .and_then(|mut file| writeln!(file, "{request}"))
            .expect("append the request that would release pending context");
        let batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &log_path,
            "codex:session:one",
            Some(old_state.clone()),
            true,
        )
        .expect("unprovable legacy state is represented as incomplete capture");

        assert!(batch.events.is_empty());
        assert_eq!(batch.consumed_bytes, 0);
        assert_eq!(batch.next.status, "incomplete");
        assert_eq!(
            batch.next.reason.as_deref(),
            Some("provider_log_legacy_pending_row_offset_unavailable")
        );
        assert_eq!(batch.next.committed_offset, original_offset);
        assert_eq!(batch.previous, Some(old_state));
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
            legacy_provider_log_event_id(&legacy_event, &log_path)
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
    fn claude_provider_log_ids_alias_only_native_event_id_rows() {
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
        assert!(events[0].metadata["legacy_event_ids"]
            .as_array()
            .is_some_and(|ids| ids.len() == 1 && ids[0].as_str() != Some(events[0].id.as_str())));
        assert!(events[2].metadata["legacy_event_ids"]
            .as_array()
            .is_some_and(|ids| ids.len() == 1 && ids[0].as_str() != Some(events[2].id.as_str())));
        assert!(events[1].metadata.get("legacy_event_ids").is_none());
        assert!(events[3].metadata.get("legacy_event_ids").is_none());
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
        for (index, event) in events.into_iter().enumerate() {
            assert_eq!(event.role, Some(AgentChatRole::System));
            assert!(matches!(
                event.metadata["input_origin"].as_str(),
                Some("context_injection" | "provider_internal")
            ));
            if index == 0 {
                let legacy_id = event
                    .metadata
                    .get("legacy_event_ids")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|ids| ids.first())
                    .and_then(serde_json::Value::as_str)
                    .expect("UUID-qualified pre-migration role alias");
                let mut pre_migration_event = event.clone();
                pre_migration_event.role = Some(AgentChatRole::User);
                assert_eq!(
                    legacy_id,
                    legacy_provider_log_event_id(&pre_migration_event, &log_path)
                );
                assert_ne!(legacy_id, event.id);
            } else {
                assert!(event.metadata.get("legacy_event_ids").is_none());
            }
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
            metadata: serde_json::json!({
                "provider_log": true,
                "log_path": "<codex-log>",
                "provider_turn_id": "provider-turn",
            }),
        };
        let mut completed = first.clone();
        completed.id = "agent-1:provider:2".to_string();
        completed.text = Some(
        "Created #daily-task-list under General.\n\n<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-1|note=[internal]\n</citation_entries>\n</oai-mem-citation>".to_string(),
    );
        completed.turn_id =
            Some("msg_003d4bf15d017fea016a460ea8668481938d3c49f567fe9108".to_string());
        completed.source = Some("response_item".to_string());
        completed.metadata["provider_phase"] = serde_json::json!("final_answer");

        // Visible text alone cannot bind two native observations to one turn.
        let unbound = merge_chat_events(Vec::new(), vec![first.clone(), completed.clone()]);
        assert_eq!(unbound.len(), 2);
        assert_eq!(unbound[0].id, first.id);
        assert_eq!(unbound[1].id, completed.id);
        assert!(unbound
            .iter()
            .all(|event| event.text.as_deref() == Some("Created #daily-task-list under General.")));

        let mut first = first;
        for event in [&mut first, &mut completed] {
            event.metadata["log_path"] = serde_json::json!("codex-session.jsonl");
            event.metadata["provider_session_id"] = serde_json::json!("provider-session");
            event.metadata["provider_turn_id"] = serde_json::json!("provider-turn");
        }
        first.metadata["raw_type"] = serde_json::json!("agent_message");
        completed.metadata["raw_type"] = serde_json::json!("message");
        completed.metadata["provider_phase"] = serde_json::json!("final_answer");

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
        // The mirror exception must not collapse two identified observations or
        // equal answers belonging to different native requests.
        let mut identified = chat_events[0].clone();
        identified.id = "other-identified-answer".into();
        identified.turn_id = Some("other-native-message".into());
        assert_eq!(
            merge_chat_events(Vec::new(), vec![chat_events[0].clone(), identified]).len(),
            2
        );
        let mut rooted_stream = chat_events[0].clone();
        rooted_stream.id = "stream-another-request".into();
        rooted_stream.turn_id = None;
        rooted_stream.source = Some("event_msg".into());
        rooted_stream.metadata["raw_type"] = serde_json::json!("agent_message");
        rooted_stream.metadata["request_root_id"] = serde_json::json!("request-a");
        rooted_stream.metadata["provider_turn_id"] = serde_json::json!("provider-turn-a");
        let mut rooted_completion = chat_events[0].clone();
        rooted_completion.metadata["request_root_id"] = serde_json::json!("request-b");
        rooted_completion.metadata["provider_turn_id"] = serde_json::json!("provider-turn-b");
        assert_eq!(
            merge_chat_events(Vec::new(), vec![rooted_stream, rooted_completion]).len(),
            2
        );
    }

    #[test]
    fn codex_four_row_watch_native_shape_coalesces_before_stream_pair() {
        let task_started =
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"provider-turn"}}"#;
        let response_item = r#"{"type":"response_item","payload":{"type":"message","id":"msg-native","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"same final answer"}],"internal_chat_message_metadata_passthrough":{"turn_id":"provider-turn"}}}"#;
        let event_message = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"same final answer"}}"#;

        let mut watch_binding = crate::providers::transcript::CodexWatchBindingState::default();
        watch_binding.set_source("provider-session", "codex-session.jsonl");
        watch_binding.observe_record(
            task_started,
            Some(&wardian_core::models::AgentEvent::TurnStarted {
                turn_id: "provider-turn".to_string(),
            }),
        );
        let watch_response = watch_binding
            .extract_message(response_item)
            .expect("watch response item");
        let watch_event = watch_binding
            .extract_message(event_message)
            .expect("watch event message");
        assert_eq!(watch_response.turn_id.as_deref(), Some("msg-native"));
        assert_eq!(watch_event.turn_id, None);

        let watch_transcript = WatchTranscript {
            cursor: "agent-1:0000000000000004".to_string(),
            messages: vec![watch_response.clone(), watch_event.clone()],
            latest_text: "same final answer".to_string(),
            truncated: false,
            omitted_bytes: 0,
        };
        let watch_events = vec![
            message_event_from_transcript(
                "agent-1",
                "codex",
                1,
                &watch_response,
                &watch_transcript,
            ),
            message_event_from_transcript("agent-1", "codex", 2, &watch_event, &watch_transcript),
        ];
        assert!(watch_events
            .iter()
            .all(|event| event.metadata["provider_log"] == true));

        let mut provider_events = normalize_chat_lines(
            "agent-1",
            "codex",
            [
                r#"{"type":"turn_context","payload":{"turn_id":"provider-turn"}}"#,
                event_message,
                response_item,
            ],
        )
        .into_iter()
        .filter(|event| event.role == Some(AgentChatRole::Assistant))
        .collect::<Vec<_>>();
        for event in &mut provider_events {
            event.metadata["provider_log"] = serde_json::json!(true);
            event.metadata["provider_session_id"] = serde_json::json!("provider-session");
            event.metadata["log_path"] = serde_json::json!("codex-session.jsonl");
        }
        assert_eq!(provider_events.len(), 2);

        let mut distinct_watch = vec![watch_events[0].clone()];
        let mut distinct_native = vec![provider_events[1].clone()];
        distinct_watch[0].turn_id = Some("msg-other".to_string());
        distinct_watch[0].metadata["provider_turn_id"] = serde_json::json!("other-turn");
        coalesce_codex_watch_observations(&mut distinct_watch, &mut distinct_native);
        assert_eq!(distinct_watch.len(), 1);
        assert_eq!(distinct_native.len(), 1);

        let native_ids = provider_events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        let watch_ids = watch_events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        let chat_events = merge_chat_events(watch_events, provider_events);
        let assistants = chat_events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .collect::<Vec<_>>();

        assert_eq!(assistants.len(), 1);
        assert_eq!(assistants[0].turn_id.as_deref(), Some("msg-native"));
        let observation_ids = assistants[0].metadata["provider_observation_ids"]
            .as_array()
            .expect("all four observation IDs retained")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(observation_ids.len(), 4);
        assert!(native_ids
            .iter()
            .chain(watch_ids.iter())
            .all(|id| observation_ids.contains(&id.as_str())));
    }

    /// Sanitized observed shape: the stream row lacks turn metadata; the
    /// response row carries the provider turn also named by the preceding context.
    fn codex_commentary_fixture_lines(reverse: bool) -> Vec<String> {
        let context = serde_json::json!({
            "type": "turn_context",
            "payload": { "turn_id": "commentary-provider-turn" }
        });
        let stream = serde_json::json!({
            "timestamp": "2026-10-24T17:38:18.275Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "phase": "commentary",
                "message": "A source-bound commentary observation."
            }
        });
        let response_item = serde_json::json!({
            "timestamp": "2026-10-24T17:38:18.275Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "id": "msg-commentary-native",
                "role": "assistant",
                "phase": "commentary",
                "content": [{
                    "type": "output_text",
                    "text": "A source-bound commentary observation."
                }],
                "internal_chat_message_metadata_passthrough": {
                    "turn_id": "commentary-provider-turn",
                    "create_time": "2026-10-24T17:38:18.275Z",
                    "content_item_kinds": ["unknown"]
                }
            }
        });
        let pair = if reverse {
            [response_item, stream]
        } else {
            [stream, response_item]
        };
        std::iter::once(context)
            .chain(pair)
            .map(|value| value.to_string())
            .collect()
    }

    fn codex_commentary_native_events(
        agent_id: &str,
        path: &Path,
        reverse: bool,
    ) -> Vec<AgentChatEvent> {
        let lines = codex_commentary_fixture_lines(reverse);
        std::fs::write(path, format!("{}\n", lines.join("\n")))
            .expect("write sanitized commentary source");
        let mut events = normalize_chat_lines(agent_id, "codex", &lines)
            .into_iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .collect::<Vec<_>>();
        decorate_forward_provider_log_events(&mut events, "codex", path);
        assert_eq!(events.len(), 2, "both native observations remain available");
        events
    }

    fn assert_unqualified_commentary_observations_preserved(
        expected: &[AgentChatEvent],
        actual: &[AgentChatEvent],
        context: &str,
    ) {
        assert_eq!(expected.len(), 2, "{context}: fixture pair");
        let expected_ids = expected
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>();
        assert_ne!(
            expected_ids[0], expected_ids[1],
            "{context}: distinct raw IDs"
        );
        let actual_ids = actual
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            actual_ids, expected_ids,
            "{context}: preserve each raw ID once and in order"
        );

        for (original, restored) in expected.iter().zip(actual) {
            assert_eq!(restored.provider, original.provider, "{context}: provider");
            assert_eq!(restored.kind, original.kind, "{context}: kind");
            assert_eq!(restored.role, original.role, "{context}: role");
            assert_eq!(
                restored.source, original.source,
                "{context}: raw source type"
            );
            assert_eq!(restored.text, original.text, "{context}: raw text");
            assert_eq!(
                restored.turn_id, original.turn_id,
                "{context}: native item ID"
            );
            assert_eq!(original.metadata["provider_phase"], "commentary");
            assert_eq!(original.metadata["provider_log"], true);
            for key in [
                "provider_phase",
                "provider_turn_id",
                "provider_session_id",
                "provider_log",
                "log_path",
                "chat_compatibility_legacy_id",
                "chat_compatibility_source_sequence",
            ] {
                assert_eq!(
                    restored.metadata.get(key),
                    original.metadata.get(key),
                    "{context}: preserve {key} provenance"
                );
            }
            for key in [
                "chat_source_ref",
                "chat_source_start",
                "chat_source_end",
                "chat_source_epoch",
            ] {
                assert!(
                    original
                        .metadata
                        .get(key)
                        .is_none_or(|value| value.is_null())
                        && restored
                            .metadata
                            .get(key)
                            .is_none_or(|value| value.is_null()),
                    "{context}: {key} is not qualified by this fixture"
                );
            }
        }
    }

    async fn insert_codex_archive_test_agent(
        state: &AppState,
        agent_id: &str,
        home: &Path,
        log_path: &Path,
    ) {
        state.agents.lock().await.insert(
            agent_id.to_string(),
            crate::state::ActiveAgent {
                config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                    session_id: agent_id.to_string(),
                    session_name: agent_id.to_string(),
                    agent_class: "Coder".to_string(),
                    provider: "codex".to_string(),
                    folder: home.to_string_lossy().to_string(),
                    fresh_provider_session_id: Some(format!("{agent_id}-codex-session")),
                    conversation_logging: AgentConversationLoggingSetting::Default,
                    ..Default::default()
                })),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: None,
                process_id: None,
                query_count: Arc::new(Mutex::new(0)),
                init_timestamp: Arc::new(Mutex::new(None)),
                last_query_timestamp: Arc::new(Mutex::new(None)),
                current_status: Arc::new(Mutex::new("Idle".to_string())),
                last_status_at: Arc::new(Mutex::new(None)),
                watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                    agent_id.to_string(),
                    32,
                    4096,
                ))),
                terminal_title: Arc::new(Mutex::new(String::new())),
                last_output_at: Arc::new(Mutex::new(None)),
                log_path: Arc::new(Mutex::new(Some(log_path.to_path_buf()))),
                log_last_modified: Arc::new(Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );
    }

    async fn persisted_chat_events_for_test_agent(
        state: &AppState,
        agent_id: &str,
    ) -> Vec<AgentChatEvent> {
        let snapshot = agent_archive_capture_snapshot(state, agent_id)
            .await
            .expect("agent capture snapshot");
        let context = conversation_archive_context_from_snapshot(&snapshot);
        state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read persisted provider observations")
    }

    #[test]
    fn codex_commentary_normalizer_preserves_phase_and_response_turn_metadata() {
        let lines = codex_commentary_fixture_lines(false);
        let context: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(context["payload"]["turn_id"], "commentary-provider-turn");
        let stream: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        assert!(stream["payload"]["turn_id"].is_null());
        assert!(stream["payload"]["internal_chat_message_metadata_passthrough"].is_null());
        let response: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
        assert!(response["payload"]["turn_id"].is_null());
        let passthrough = &response["payload"]["internal_chat_message_metadata_passthrough"];
        assert_eq!(passthrough["turn_id"], "commentary-provider-turn");
        assert!(passthrough.get("create_time").is_some());
        assert_eq!(
            passthrough["content_item_kinds"],
            serde_json::json!(["unknown"])
        );

        let events = normalize_chat_lines("commentary-agent", "codex", &lines);
        let assistants = events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .collect::<Vec<_>>();
        assert_eq!(assistants.len(), 2);
        assert!(assistants[0].turn_id.is_none());
        assert_eq!(
            assistants[1].turn_id.as_deref(),
            Some("msg-commentary-native")
        );
        for event in &assistants {
            assert_eq!(event.metadata["provider_phase"], "commentary");
            assert_eq!(
                event.metadata["provider_turn_id"],
                "commentary-provider-turn"
            );
        }
    }

    #[tokio::test]
    async fn codex_commentary_raw_capture_and_reopened_query_preserves_unqualified_observations() {
        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().expect("isolated commentary archive home");
        let _home = WardianHomeGuard::set(&env_lock, home.path());
        for reverse in [false, true] {
            let agent_id = if reverse {
                "commentary-reverse"
            } else {
                "commentary-forward"
            };
            let path = home.path().join(format!("{agent_id}.jsonl"));
            let native = codex_commentary_native_events(agent_id, &path, reverse);
            let archive = crate::state::conversation_archive::ConversationArchiveState::default();
            // Normal capture persists the raw batch before live merge runs.
            archive
                .append_chat_events(agent_id, &native)
                .expect("capture raw pair");
            let current = merge_chat_events(Vec::new(), native.clone());
            let archived = archive
                .chat_events_for_agent(agent_id)
                .expect("query durable pair");
            let projected = crate::state::conversation_archive::provenance::merge_current_capture(
                current, archived,
            )
            .expect("project current capture with durable observations");
            assert_unqualified_commentary_observations_preserved(
                &native,
                &projected,
                "raw capture in either native arrival order",
            );

            let reopened = crate::state::conversation_archive::ConversationArchiveState::default();
            let replayed = reopened
                .chat_events_for_agent(agent_id)
                .expect("reopen archive");
            let projected = crate::state::conversation_archive::provenance::merge_current_capture(
                merge_chat_events(Vec::new(), native.clone()),
                replayed,
            )
            .expect("replay unqualified commentary observations");
            assert_unqualified_commentary_observations_preserved(
                &native,
                &projected,
                "reopen/replay in either native arrival order",
            );
        }
    }

    #[tokio::test]
    async fn codex_commentary_loader_persists_one_show_answer_across_reopen() {
        use std::io::Write as _;

        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().expect("isolated loader archive home");
        let _home = WardianHomeGuard::set(&env_lock, home.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("enable conversation logging for the fixture");

        const COMMENTARY: &str = "A source-bound commentary observation.";
        let agent_id = "commentary-loader";
        let log_path = home.path().join("commentary-loader.jsonl");
        const SESSION_META_LINE: usize = 0;
        const TURN_CONTEXT_LINE: usize = 1;
        const STREAM_LINE: usize = 2;
        const NATIVE_RESPONSE_LINE: usize = 3;
        let mut lines = codex_commentary_fixture_lines(false);
        // Match insert_codex_archive_test_agent's configured fresh Codex session.
        let codex_session_id = format!("{agent_id}-codex-session");
        lines.insert(
            SESSION_META_LINE,
            serde_json::json!({
                "type": "session_meta",
                "payload": { "id": codex_session_id.clone() }
            })
            .to_string(),
        );
        std::fs::write(&log_path, format!("{}\n", lines[..=STREAM_LINE].join("\n")))
            .expect("write mirror-only provider log");

        let state = AppState::new();
        insert_codex_archive_test_agent(&state, agent_id, home.path(), &log_path).await;
        let page_a_initial = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("passively read the mirror-only bounded page before capture");
        let initial_passive_capture = serde_json::to_string(&page_a_initial)
            .expect("serialize the first passive page before semantic assertions");
        eprintln!("ISSUE1693_PAGE_A_INITIAL_PASSIVE={initial_passive_capture}");
        let initial_a = page_a_initial.events.iter().find(|event| {
            event.role == Some(AgentChatRole::Assistant)
                && event.text.as_deref() == Some(COMMENTARY)
        });
        let initial_a_id = initial_a.map(|event| event.id.clone());
        let initial_a_source_ref = initial_a.map(|event| {
            event.metadata["chat_source_ref"]
                .as_str()
                .filter(|source_ref| !source_ref.trim().is_empty())
                .expect("initial passive A has a source-bound identity")
                .to_owned()
        });
        assert_eq!(page_a_initial.session_id, agent_id);
        if let Some(initial_a) = initial_a {
            assert!(
                matches!(
                    page_a_initial.progress.as_str(),
                    "ready" | "provisional" | "indexing"
                ),
                "an initial passive A row has valid recent-page progress"
            );
            assert!(!initial_a.id.trim().is_empty());
            assert_eq!(initial_a.source.as_deref(), Some("event_msg"));
            assert_eq!(initial_a.text.as_deref(), Some(COMMENTARY));
        }

        crate::commands::background_capture::capture_background_for_state(&state, agent_id)
            .await
            .expect("join the production background capture and projection owner for A");
        let page_a = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            Some(page_a_initial.revision.clone()),
            None,
            None,
        )
        .await
        .expect("refresh A through the passive bounded page after owner progress");
        let page_a_commentary_id = page_a
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .map(|event| event.id.clone());
        let canonical_a_id = page_a_commentary_id
            .clone()
            .expect("owner-qualified page A publishes the canonical commentary");
        let conversation_id_a = state
            .conversation_archive
            .active_conversation_id(agent_id)
            .expect("page A active conversation lookup")
            .expect("page A captured conversation");
        let page_a_capture = serde_json::to_string(&serde_json::json!({
            "initial_passive": &page_a_initial,
            "after_owner": &page_a,
        }))
        .expect("serialize page A ownership and reconciliation evidence");
        eprintln!("ISSUE1693_PAGE_A_AFTER_OWNER={page_a_capture}");
        assert!(
            matches!(page_a.progress.as_str(), "ready" | "provisional"),
            "page A has valid recent-page progress after the production owner"
        );
        assert!(!page_a.revision.is_empty(), "page A has an actual revision");
        assert!(
            page_a
                .generation
                .as_deref()
                .is_some_and(|generation| !generation.trim().is_empty()),
            "page A has an actual source generation"
        );
        assert!(
            page_a
                .source_epoch
                .as_deref()
                .is_some_and(|source_epoch| !source_epoch.trim().is_empty()),
            "page A has an actual source epoch"
        );
        assert_eq!(
            page_a.session_id, agent_id,
            "page A precondition: bounded page belongs to the expected agent"
        );
        assert_eq!(
            page_a.conversation_id.as_deref(),
            Some(conversation_id_a.as_str()),
            "page A precondition: ready page belongs to the active native-session conversation"
        );
        if let Some(initial_a_id) = initial_a_id.as_deref() {
            assert!(
                canonical_a_id == initial_a_id
                    || page_a.aliases.iter().any(|alias| {
                        alias.observation_id == initial_a_id
                            && alias.canonical_id == canonical_a_id
                    })
                    || page_a.reset,
                "page A reconciles its provisional observation through the page identity contract; {page_a_capture}"
            );
        }
        assert_eq!(
            archive_identity::native_log_session(&log_path, "codex"),
            Some(codex_session_id.clone()),
            "page A precondition: the provider header binds the expected native session"
        );
        let archived_a = persisted_chat_events_for_test_agent(&state, agent_id).await;
        let mirror_a = archived_a
            .iter()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("owner persisted the lossless stream observation A");
        let mirror_id = mirror_a.id.clone();
        assert_eq!(mirror_a.text.as_deref(), Some(COMMENTARY));
        if let Some(initial_a) = initial_a {
            for key in ["chat_source_start", "chat_source_end", "chat_source_epoch"] {
                assert_eq!(
                    mirror_a.metadata[key], initial_a.metadata[key],
                    "initial A {key}"
                );
            }
        }
        if let Some(initial_source_ref) = initial_a_source_ref.as_deref() {
            assert_eq!(
                mirror_a.metadata["chat_source_ref"].as_str(),
                Some(initial_source_ref),
                "initial passive A remains bound to the captured raw source"
            );
        }

        let mut provider_log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("open provider log to append native response item");
        writeln!(provider_log, "{}", lines[NATIVE_RESPONSE_LINE])
            .expect("append native response item");
        drop(provider_log);

        crate::commands::background_capture::capture_background_for_state(&state, agent_id)
            .await
            .expect("join the production background capture and projection owner after B");
        let page_b_update = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            Some(page_a.revision.clone()),
            None,
            None,
        )
        .await
        .expect("apply the actual page revision after native observation B");
        let page_b = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("read the current bounded page after applying B's revision");
        let page_b_commentary_id = page_b
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .map(|event| event.id.clone());
        let page_capture = serde_json::to_string(&serde_json::json!({
            "A_initial_passive_page": &page_a_initial,
            "A_after_background_owner": &page_a,
            "B_revision_update": &page_b_update,
            "B_current_page": &page_b,
        }))
        .expect("serialize actual bounded page evidence before semantic assertions");
        eprintln!("ISSUE1693_LOADER_PAGE_AB={page_capture}");
        let assistants_b = page_b
            .events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .collect::<Vec<_>>();

        let archived_events = persisted_chat_events_for_test_agent(&state, agent_id).await;
        let mirror = archived_events
            .iter()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("lossless archived stream observation");
        let native = archived_events
            .iter()
            .find(|event| event.source.as_deref() == Some("response_item"))
            .expect("lossless archived native response observation");
        assert_eq!(
            mirror.id, mirror_id,
            "A keeps its actual raw observation ID"
        );

        let conversation_id = state
            .conversation_archive
            .active_conversation_id(agent_id)
            .expect("active conversation lookup")
            .expect("conversation created by normal capture");
        let (manifest, records) = state
            .conversation_archive
            .show(&conversation_id)
            .expect("show persisted canonical conversation");
        let conversation_dir =
            wardian_core::paths::agent_conversation_dir(agent_id, &conversation_id)
                .expect("physical conversation directory");
        let physical_records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("conversation.jsonl"),
            )
            .expect("read physical canonical narrative rows");
        let physical_manifest: wardian_core::conversations::ConversationManifest =
            serde_json::from_slice(
                &std::fs::read(conversation_dir.join("manifest.json"))
                    .expect("read physical conversation manifest"),
            )
            .expect("parse physical conversation manifest");
        let physical_index_path = wardian_core::paths::agent_conversations_dir(agent_id)
            .expect("agent conversations directory")
            .join("index.jsonl");
        let physical_index =
            wardian_core::conversations::read_latest_index_entries(&physical_index_path)
                .expect("read physical agent conversation index")
                .into_iter()
                .find(|entry| entry.conversation_id == conversation_id)
                .expect("indexed active conversation");
        let physical_sources: Vec<wardian_core::conversations::ConversationSourceRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("sources.jsonl"),
            )
            .expect("read physical provider source rows");
        let native_log_bytes = std::fs::read(&log_path).expect("read exact native provider rows");

        let evidence_capture = serde_json::to_string(&serde_json::json!({
            "page_a_initial_passive": &page_a_initial,
            "page_a_before_native_append": &page_a,
            "page_b_revision_update": &page_b_update,
            "page_b_after_native_append": &page_b,
            "codex_session_meta_id": &codex_session_id,
            "conversation_id_a": &conversation_id_a,
            "conversation_id_after_b": &conversation_id,
            "page_a_commentary_id": &page_a_commentary_id,
            "page_b_commentary_id": &page_b_commentary_id,
            "archive_raw_event_ids": archived_events.iter().map(|event| &event.id).collect::<Vec<_>>(),
            "archive_source_refs": archived_events.iter()
                .filter_map(|event| event.metadata["chat_source_ref"].as_str())
                .collect::<Vec<_>>(),
            "show": {
                "record_count": manifest.record_count,
                "event_refs": records.iter().map(|record| &record.event_refs).collect::<Vec<_>>(),
                "source_refs": records.iter().map(|record| &record.source_refs).collect::<Vec<_>>(),
            },
            "physical": {
                "conversation_record_count": physical_records.len(),
                "event_refs": physical_records.iter().map(|record| &record.event_refs).collect::<Vec<_>>(),
                "source_refs": physical_records.iter().map(|record| &record.source_refs).collect::<Vec<_>>(),
                "manifest_record_count": physical_manifest.record_count,
                "index_record_count": physical_index.record_count,
                "source_ids": physical_sources.iter().map(|source| &source.source_id).collect::<Vec<_>>(),
                "source_records": &physical_sources,
            },
        }))
        .expect("serialize actual B1 page and durable archive evidence");
        eprintln!("ISSUE1693_B1_EVIDENCE={evidence_capture}");

        assert_eq!(
            archive_identity::native_log_session(&log_path, "codex"),
            Some(codex_session_id.clone()),
            "B1 precondition: the provider header still binds the expected native session; {evidence_capture}"
        );
        assert!(
            matches!(page_b.progress.as_str(), "ready" | "provisional"),
            "page B has valid recent-page progress"
        );
        assert!(!page_b.revision.is_empty(), "page B has an actual revision");
        assert_eq!(
            page_b.session_id, agent_id,
            "page B ownership precondition: bounded page belongs to the expected agent"
        );
        assert_eq!(
            page_b.conversation_id.as_deref(),
            Some(conversation_id.as_str()),
            "page B ownership precondition: page belongs to the active native session"
        );
        assert_eq!(
            conversation_id, conversation_id_a,
            "page B ownership precondition: native append remains in page A's conversation"
        );
        let canonical_b_id = page_b_commentary_id
            .clone()
            .expect("page B publishes the paired commentary");
        assert!(
            page_b_update.reset
                || canonical_b_id == canonical_a_id
                || (page_b_update
                    .events
                    .iter()
                    .any(|event| event.id == canonical_b_id)
                    && page_b_update.aliases.iter().any(|alias| {
                        alias.observation_id == canonical_a_id
                            && alias.canonical_id == canonical_b_id
                    })),
            "the supplied revision preserves A through its actual B alias, unchanged identity, or reset; {evidence_capture}"
        );
        assert!(!page_b_update.removed_ids.contains(&canonical_a_id));
        assert_eq!(records.len(), physical_records.len());
        assert_eq!(
            manifest.record_count,
            u64::try_from(records.len()).expect("manifest count conversion")
        );
        assert_eq!(physical_manifest.record_count, manifest.record_count);
        assert_eq!(physical_index.record_count, manifest.record_count);
        assert_eq!(
            assistants_b.len(),
            1,
            "one current bounded-page answer; actual page A/B: {page_capture}"
        );
        let display_member_ids = |event: &AgentChatEvent| {
            event.metadata["chat_display_member_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_str().map(|id| id.to_string()))
                .collect::<Vec<_>>()
        };
        let assert_observation_durable =
            |observation: &AgentChatEvent,
             show_records: &[wardian_core::conversations::ConversationNarrativeRecord],
             physical_records: &[wardian_core::conversations::ConversationNarrativeRecord],
             archive_sources: &[wardian_core::conversations::ConversationSourceRecord],
             persisted_events: &[AgentChatEvent],
             native_log_bytes: &[u8]| {
                // records.rs hashes the raw event ID into the archive source ID;
                // chat_source_ref separately binds that event to its native log row.
                assert_eq!(observation.provider, "codex");
                let digest = Sha256::digest(observation.id.as_bytes());
                let digest_prefix = digest
                    .iter()
                    .take(12)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join("");
                let archive_source_id = format!("src_codex_observation_{digest_prefix}");
                let source_rows = archive_sources
                    .iter()
                    .filter(|source| source.source_id == archive_source_id)
                    .collect::<Vec<_>>();
                assert_eq!(
                    source_rows.len(),
                    1,
                    "one physical archive source row binds raw event {}; sources={}",
                    observation.id,
                    serde_json::to_string(archive_sources).expect("serialize source evidence")
                );
                let source = source_rows[0];
                assert_eq!(source.provider, observation.provider);
                assert_eq!(
                    source.source_kind,
                    observation
                        .source
                        .as_deref()
                        .expect("Codex source kind used by the archive constructor")
                );
                let metadata_string = |key: &str| match observation.metadata.get(key)? {
                    serde_json::Value::String(value) => {
                        let trimmed = value.trim();
                        (!trimmed.is_empty()).then(|| trimmed.to_owned())
                    }
                    serde_json::Value::Number(value) => Some(value.to_string()),
                    serde_json::Value::Bool(value) => Some(value.to_string()),
                    _ => None,
                };
                let expected_provider_session_id = metadata_string("opencode_session_id")
                    .or_else(|| metadata_string("provider_session_id"));
                assert_eq!(
                    source.provider_session_id, expected_provider_session_id,
                    "archive source session follows the constructor's optional event metadata"
                );
                let expected_source_path = observation
                    .path
                    .clone()
                    .or_else(|| metadata_string("source_path"))
                    .or_else(|| metadata_string("path"))
                    .or_else(|| metadata_string("log_path"));
                assert_eq!(source.source_path, expected_source_path);

                let raw_matches = persisted_events
                    .iter()
                    .filter(|event| event.id == observation.id)
                    .collect::<Vec<_>>();
                assert_eq!(
                    raw_matches.len(),
                    1,
                    "raw event ID remains unique in archive"
                );
                let persisted = raw_matches[0];
                assert_eq!(persisted.metadata["provider_log"], true);
                for key in [
                    "chat_source_ref",
                    "chat_source_start",
                    "chat_source_end",
                    "chat_source_epoch",
                    PROVIDER_LOG_ROW_OFFSET_METADATA_KEY,
                ] {
                    assert_eq!(persisted.metadata[key], observation.metadata[key], "{key}");
                }
                let start = persisted.metadata["chat_source_start"]
                    .as_u64()
                    .expect("native source start byte");
                let end = persisted.metadata["chat_source_end"]
                    .as_u64()
                    .expect("native source end byte");
                let start_index = usize::try_from(start).expect("native source start fits memory");
                let end_index = usize::try_from(end).expect("native source end fits memory");
                let framed_row = native_log_bytes
                    .get(start_index..end_index)
                    .expect("native source range addresses the provider log");
                assert!(
                    framed_row.ends_with(b"\n"),
                    "native source range includes its row newline"
                );
                let raw_line = std::str::from_utf8(framed_row)
                    .expect("provider row is UTF-8")
                    .trim_end_matches(['\r', '\n']);
                let raw_value: serde_json::Value =
                    serde_json::from_str(raw_line).expect("native source range is one JSON row");
                match persisted.source.as_deref() {
                    Some("event_msg") => assert_eq!(
                        raw_value["payload"]["message"].as_str(),
                        persisted.text.as_deref(),
                        "stream event is bound to its actual native row"
                    ),
                    Some("response_item") => assert_eq!(
                        raw_value["payload"]["id"].as_str(),
                        persisted.turn_id.as_deref(),
                        "native item ID is bound to its actual native row"
                    ),
                    other => panic!("unexpected Codex observation source {other:?}"),
                }
                let source_ref = persisted.metadata["chat_source_ref"]
                    .as_str()
                    .expect("raw observation source coordinate");
                let ordinal = source_ref
                    .rsplit(':')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .expect("native source reference carries its row ordinal");
                let mut row_ordinals = persisted_events
                    .iter()
                    .filter(|event| {
                        event.metadata["provider_log"] == true
                            && event.metadata["chat_source_start"].as_u64() == Some(start)
                            && event.metadata["chat_source_end"].as_u64() == Some(end)
                    })
                    .map(|event| {
                        event.metadata["chat_source_ref"]
                            .as_str()
                            .and_then(|source_ref| source_ref.rsplit(':').next())
                            .and_then(|value| value.parse::<usize>().ok())
                            .expect("each observation on a native row has an ordinal")
                    })
                    .collect::<Vec<_>>();
                row_ordinals.sort_unstable();
                assert_eq!(
                    row_ordinals,
                    (0..row_ordinals.len()).collect::<Vec<_>>(),
                    "native row ordinals are unique and recoverable"
                );
                assert!(row_ordinals.contains(&ordinal));
                let identity = crate::commands::provider_log_acquisition::native_file_identity(
                    &std::fs::File::open(&log_path).expect("open native provider log identity"),
                )
                .expect("read native provider file identity");
                assert_eq!(
                    source_ref,
                    super::super::chat_recent_seed::source_reference(
                        agent_id, &identity, start, raw_line, ordinal,
                    ),
                    "native source reference recovers the exact row, file identity, and ordinal"
                );
                let expected_epoch = super::super::chat_recent_seed::hash(
                    &serde_json::to_vec(&identity).expect("serialize native file identity"),
                );
                assert_eq!(
                    persisted.metadata["chat_source_epoch"].as_str(),
                    Some(expected_epoch.as_str())
                );

                for (surface, rows) in [("show", show_records), ("physical", physical_records)] {
                    let owners = rows
                        .iter()
                        .filter(|record| record.event_refs.contains(&observation.id))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        owners.len(),
                        1,
                        "{surface} has one narrative owner for raw observation {}",
                        observation.id
                    );
                    let record = owners[0];
                    assert_eq!(
                        record.text.as_deref().or(record.excerpt.as_deref()),
                        observation.text.as_deref(),
                        "{surface} retains raw observation text for {}",
                        observation.id
                    );
                    assert_eq!(
                        record
                            .source_refs
                            .iter()
                            .filter(|source_id| source_id.as_str() == archive_source_id.as_str())
                            .count(),
                        1,
                        "{surface} joins raw event {} to archive source ID {archive_source_id}",
                        observation.id
                    );
                    assert_eq!(
                    rows.iter()
                        .filter(|candidate| {
                            candidate.source_refs.iter().any(|source_id| {
                                source_id.as_str() == archive_source_id.as_str()
                            })
                        })
                        .count(),
                    1,
                    "{surface} has one narrative reference to archive source ID {archive_source_id}"
                );
                }
            };
        let paired = assistants_b[0];
        let paired_observation_ids = display_member_ids(paired);
        assert_ne!(
            mirror.id, native.id,
            "the two provider observations keep distinct IDs"
        );
        assert_eq!(paired_observation_ids.len(), 2);
        assert!(paired_observation_ids.contains(&mirror.id));
        assert!(paired_observation_ids.contains(&native.id));
        assert_eq!(mirror.turn_id, None);
        assert_eq!(native.turn_id.as_deref(), Some("msg-commentary-native"));
        assert_eq!(mirror.text.as_deref(), Some(COMMENTARY));
        assert_eq!(mirror.metadata["provider_phase"], "commentary");
        for observation in [mirror, native] {
            assert_eq!(observation.text.as_deref(), Some(COMMENTARY));
            assert_eq!(
                observation.metadata["provider_turn_id"],
                "commentary-provider-turn"
            );
            assert_eq!(
                observation.metadata["log_path"].as_str(),
                Some(log_path.to_string_lossy().as_ref())
            );
        }
        assert_eq!(native.metadata["provider_phase"], "commentary");
        assert_eq!(
            paired.id, canonical_b_id,
            "the actual post-B page identity owns the paired commentary"
        );
        for observation in [mirror, native] {
            assert_observation_durable(
                observation,
                &records,
                &physical_records,
                &physical_sources,
                &archived_events,
                &native_log_bytes,
            );
        }

        let reopened = AppState::new();
        insert_codex_archive_test_agent(&reopened, agent_id, home.path(), &log_path).await;
        crate::commands::background_capture::capture_background_for_state(&reopened, agent_id)
            .await
            .expect("join the production owner after reopening the paired archive");
        let (_, reopen_records) = reopened
            .conversation_archive
            .show(&conversation_id)
            .expect("show the paired archive after first reopen");
        let reopen_physical_records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("conversation.jsonl"),
            )
            .expect("read physical rows after first reopen");
        let reopen_sources: Vec<wardian_core::conversations::ConversationSourceRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("sources.jsonl"),
            )
            .expect("read physical source rows after first reopen");
        let reopen_native_log_bytes =
            std::fs::read(&log_path).expect("read native provider rows after first reopen");
        let reopen_source_capture = serde_json::to_string(&reopen_sources)
            .expect("serialize complete source rows after first reopen");
        eprintln!("ISSUE1693_B1_SOURCES_REOPEN={reopen_source_capture}");
        let reopened_page = load_agent_chat_page_with_revalidation_for_state(
            &reopened,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("publish the paired bounded page from a fresh AppState");
        let reopened_page_pair = reopened_page
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .expect("reopened page contains the canonical commentary");
        assert!(
            matches!(reopened_page.progress.as_str(), "ready" | "provisional"),
            "reopened page has valid recent-page progress"
        );
        assert!(
            !reopened_page.revision.is_empty(),
            "reopened page has an actual revision"
        );
        assert_eq!(reopened_page.session_id, agent_id);
        assert_eq!(
            reopened_page.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert_eq!(
            reopened_page_pair.id, canonical_b_id,
            "fresh-state page retains the post-B canonical identity"
        );
        assert_eq!(
            reopened_page
                .events
                .iter()
                .filter(|event| {
                    event.role == Some(AgentChatRole::Assistant)
                        && event.text.as_deref() == Some(COMMENTARY)
                })
                .count(),
            1,
            "reopened bounded page retains one canonical pair"
        );
        let reopened_observation_ids = display_member_ids(reopened_page_pair);
        assert!(reopened_observation_ids.contains(&mirror.id));
        assert!(reopened_observation_ids.contains(&native.id));
        let reopened_raw_events = persisted_chat_events_for_test_agent(&reopened, agent_id).await;
        assert!(reopened_raw_events
            .iter()
            .any(|event| event.id.as_str() == mirror.id.as_str()));
        assert!(reopened_raw_events
            .iter()
            .any(|event| event.id.as_str() == native.id.as_str()));
        for original in [mirror, native] {
            let restored = reopened_raw_events
                .iter()
                .find(|event| event.id == original.id)
                .expect("raw provider observation survives archive reopen");
            for key in [
                "chat_source_ref",
                "chat_source_start",
                "chat_source_end",
                "chat_source_epoch",
                "provider_log_row_offset",
            ] {
                assert_eq!(restored.metadata[key], original.metadata[key], "{key}");
            }
        }
        for observation in [mirror, native] {
            assert_observation_durable(
                observation,
                &reopen_records,
                &reopen_physical_records,
                &reopen_sources,
                &reopened_raw_events,
                &reopen_native_log_bytes,
            );
        }

        crate::commands::background_capture::capture_background_for_state(&reopened, agent_id)
            .await
            .expect("repeat the production owner to verify idempotent reopen");
        let repeated_update = load_agent_chat_page_with_revalidation_for_state(
            &reopened,
            agent_id.to_string(),
            None,
            Some(reopened_page.revision.clone()),
            None,
            None,
        )
        .await
        .expect("refresh the reopened page using its actual revision");
        let repeated_page = load_agent_chat_page_with_revalidation_for_state(
            &reopened,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("read the current reopened bounded page after the revision update");
        let repeated = repeated_page
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .expect("repeated canonical assistant");
        assert_eq!(
            repeated.id, canonical_b_id,
            "repeat load keeps the post-B identity"
        );
        assert_eq!(
            display_member_ids(repeated),
            paired_observation_ids,
            "repeat load keeps both provider observation IDs"
        );
        assert!(!repeated_update.removed_ids.contains(&canonical_b_id));
        let (_, repeated_records) = reopened
            .conversation_archive
            .show(&conversation_id)
            .expect("show after repeated capture");
        let repeated_physical_records: Vec<
            wardian_core::conversations::ConversationNarrativeRecord,
        > = wardian_core::conversations::read_jsonl_records(
            &conversation_dir.join("conversation.jsonl"),
        )
        .expect("read physical rows after repeated capture");
        let repeated_sources: Vec<wardian_core::conversations::ConversationSourceRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("sources.jsonl"),
            )
            .expect("read physical source rows after repeated capture");
        let repeated_native_log_bytes =
            std::fs::read(&log_path).expect("read native provider rows after repeated capture");
        let repeated_raw_events = persisted_chat_events_for_test_agent(&reopened, agent_id).await;
        let repeated_source_capture = serde_json::to_string(&repeated_sources)
            .expect("serialize complete source rows after repeated capture");
        eprintln!("ISSUE1693_B1_SOURCES_REPEAT={repeated_source_capture}");
        for observation in [mirror, native] {
            assert_observation_durable(
                observation,
                &repeated_records,
                &repeated_physical_records,
                &repeated_sources,
                &repeated_raw_events,
                &repeated_native_log_bytes,
            );
        }

        let mut control_lines = lines
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        const CONTROL_TEXT: &str = "Two distinct native answers share this text.";
        const CONTROL_TURN: &str = "commentary-controls-turn";
        control_lines[TURN_CONTEXT_LINE]["payload"]["turn_id"] = serde_json::json!(CONTROL_TURN);
        control_lines[STREAM_LINE]["payload"]["message"] = serde_json::json!(CONTROL_TEXT);
        control_lines[NATIVE_RESPONSE_LINE]["payload"]["content"][0]["text"] =
            serde_json::json!(CONTROL_TEXT);
        control_lines[NATIVE_RESPONSE_LINE]["payload"]["id"] =
            serde_json::json!("msg-commentary-native-one");
        control_lines[NATIVE_RESPONSE_LINE]["payload"]
            ["internal_chat_message_metadata_passthrough"]["turn_id"] =
            serde_json::json!(CONTROL_TURN);
        let mut second_native = control_lines[NATIVE_RESPONSE_LINE].clone();
        second_native["payload"]["id"] = serde_json::json!("msg-commentary-native-two");
        let mut provider_log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("append ambiguous native controls to the provider log");
        for line in [
            control_lines[TURN_CONTEXT_LINE].to_string(),
            control_lines[STREAM_LINE].to_string(),
            control_lines[NATIVE_RESPONSE_LINE].to_string(),
            second_native.to_string(),
        ] {
            writeln!(provider_log, "{line}").expect("append control observation");
        }
        drop(provider_log);
        crate::commands::background_capture::capture_background_for_state(&reopened, agent_id)
            .await
            .expect("join the production owner after appending ambiguous controls");
        let controls_update = load_agent_chat_page_with_revalidation_for_state(
            &reopened,
            agent_id.to_string(),
            None,
            Some(repeated_page.revision.clone()),
            None,
            None,
        )
        .await
        .expect("apply the reopened page revision after appending controls");
        let controls_page = load_agent_chat_page_with_revalidation_for_state(
            &reopened,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("read the current bounded page containing ambiguous controls");
        let control_assistants = controls_page
            .events
            .iter()
            .filter(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(CONTROL_TEXT)
            })
            .collect::<Vec<_>>();
        assert!(
            controls_update.reset
                || controls_update
                    .events
                    .iter()
                    .any(|event| { event.text.as_deref() == Some(CONTROL_TEXT) })
                || controls_update.aliases.iter().any(|alias| {
                    control_assistants
                        .iter()
                        .any(|event| event.id == alias.canonical_id)
                }),
            "the supplied revision reports the new controls through its delta, alias, or reset"
        );
        assert_eq!(control_assistants.len(), 3);
        assert!(control_assistants.iter().any(|event| {
            event.source.as_deref() == Some("event_msg") && event.turn_id.is_none()
        }));
        let native_ids = control_assistants
            .iter()
            .filter(|event| event.source.as_deref() == Some("response_item"))
            .filter_map(|event| event.turn_id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(native_ids.len(), 2);
        assert!(native_ids.contains(&"msg-commentary-native-one"));
        assert!(native_ids.contains(&"msg-commentary-native-two"));
        assert!(control_assistants.iter().all(|event| {
            event.metadata["provider_phase"] == "commentary"
                && event.metadata["provider_turn_id"] == CONTROL_TURN
        }));
        let control_all_raw_events =
            persisted_chat_events_for_test_agent(&reopened, agent_id).await;
        let control_raw_events = control_all_raw_events
            .iter()
            .filter(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(CONTROL_TEXT)
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            control_raw_events.len(),
            3,
            "all raw controls remain persisted"
        );
        let (_, control_records) = reopened
            .conversation_archive
            .show(&conversation_id)
            .expect("show preserved control observations");
        let control_physical_records: Vec<
            wardian_core::conversations::ConversationNarrativeRecord,
        > = wardian_core::conversations::read_jsonl_records(
            &conversation_dir.join("conversation.jsonl"),
        )
        .expect("read physical rows after preserving ambiguous controls");
        let control_sources: Vec<wardian_core::conversations::ConversationSourceRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("sources.jsonl"),
            )
            .expect("read physical source rows after preserving ambiguous controls");
        let control_native_log_bytes =
            std::fs::read(&log_path).expect("read native provider rows after controls");
        let control_source_capture = serde_json::to_string(&control_sources)
            .expect("serialize complete source rows after controls");
        eprintln!("ISSUE1693_B1_SOURCES_CONTROLS={control_source_capture}");
        let control_owners = control_raw_events
            .iter()
            .map(|event| {
                let matches = control_records
                    .iter()
                    .enumerate()
                    .filter(|(_, record)| record.event_refs.contains(&event.id))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                assert_eq!(matches.len(), 1, "each raw control has one narrative owner");
                matches[0]
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            control_owners.len(),
            3,
            "ambiguous controls remain distinct"
        );
        for observation in [mirror, native]
            .into_iter()
            .chain(control_raw_events.iter())
        {
            assert_observation_durable(
                observation,
                &control_records,
                &control_physical_records,
                &control_sources,
                &control_all_raw_events,
                &control_native_log_bytes,
            );
        }

        let mut provider_log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("append enough source rows to traverse the bounded page");
        for index in 0..80 {
            let line = serde_json::json!({
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "turn_id": format!("page-turn-{index}"),
                    "message": format!("Bounded page history {index}"),
                }
            });
            writeln!(provider_log, "{line}").expect("append bounded page history row");
        }
        drop(provider_log);

        crate::commands::background_capture::capture_background_for_state(&reopened, agent_id)
            .await
            .expect("join the production owner after appending bounded-page history");

        let final_reopened = AppState::new();
        insert_codex_archive_test_agent(&final_reopened, agent_id, home.path(), &log_path).await;
        crate::commands::background_capture::capture_background_for_state(
            &final_reopened,
            agent_id,
        )
        .await
        .expect("join the production owner after the post-control AppState reopen");
        let head_page = load_agent_chat_page_with_revalidation_for_state(
            &final_reopened,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("ordinary bounded head page after reopen");
        assert!(
            matches!(head_page.progress.as_str(), "ready" | "provisional"),
            "head page has valid recent-page progress"
        );
        assert!(
            !head_page.revision.is_empty(),
            "head page has an actual revision"
        );
        assert_eq!(head_page.session_id, agent_id);
        assert_eq!(
            head_page.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        let (final_show_manifest, reopened_records) = final_reopened
            .conversation_archive
            .show(&conversation_id)
            .expect("show the same archive after AppState reopen");
        let final_conversation_dir =
            wardian_core::paths::agent_conversation_dir(agent_id, &conversation_id)
                .expect("reopened physical conversation directory");
        let final_physical_records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &final_conversation_dir.join("conversation.jsonl"),
            )
            .expect("read reopened physical canonical rows");
        let final_sources: Vec<wardian_core::conversations::ConversationSourceRecord> =
            wardian_core::conversations::read_jsonl_records(
                &final_conversation_dir.join("sources.jsonl"),
            )
            .expect("read reopened physical source rows");
        let final_native_log_bytes =
            std::fs::read(&log_path).expect("read native provider rows after final reopen");
        let final_source_capture = serde_json::to_string(&final_sources)
            .expect("serialize complete source rows after final reopen");
        eprintln!("ISSUE1693_B1_SOURCES_FINAL={final_source_capture}");
        assert_eq!(final_physical_records.len(), reopened_records.len());
        let final_manifest: wardian_core::conversations::ConversationManifest =
            serde_json::from_slice(
                &std::fs::read(final_conversation_dir.join("manifest.json"))
                    .expect("read reopened physical manifest"),
            )
            .expect("parse reopened physical manifest");
        assert_eq!(
            final_show_manifest.record_count,
            u64::try_from(reopened_records.len()).expect("show count conversion")
        );
        assert_eq!(
            final_manifest.record_count,
            final_show_manifest.record_count
        );
        let final_index = wardian_core::conversations::read_latest_index_entries(
            &wardian_core::paths::agent_conversations_dir(agent_id)
                .expect("reopened agent conversations directory")
                .join("index.jsonl"),
        )
        .expect("read reopened physical conversation index")
        .into_iter()
        .find(|entry| entry.conversation_id == conversation_id)
        .expect("reopened conversation index entry");
        assert_eq!(final_index.record_count, final_show_manifest.record_count);
        let final_raw_events =
            persisted_chat_events_for_test_agent(&final_reopened, agent_id).await;
        for original in [mirror, native] {
            let restored = final_raw_events
                .iter()
                .find(|event| event.id == original.id)
                .expect("both raw provider observations survive final reopen");
            for key in [
                "chat_source_ref",
                "chat_source_start",
                "chat_source_end",
                "chat_source_epoch",
                "provider_log_row_offset",
            ] {
                assert_eq!(restored.metadata[key], original.metadata[key], "{key}");
            }
        }
        for observation in [mirror, native] {
            assert_observation_durable(
                observation,
                &reopened_records,
                &final_physical_records,
                &final_sources,
                &final_raw_events,
                &final_native_log_bytes,
            );
        }
        let remote_page = crate::remote::operations::remote_agent_chat_page_with_revalidation(
            &final_reopened,
            agent_id,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("ordinary remote page uses the shared loader without IPC");
        let head_ids = head_page
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        let remote_head_ids = remote_page
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(remote_head_ids, head_ids);
        assert!(
            head_page.next_before.is_some(),
            "head page exposes older history"
        );
        let expected_history_texts = (0..80)
            .map(|index| format!("Bounded page history {index}"))
            .collect::<std::collections::BTreeSet<_>>();
        let head_history_texts = head_page
            .events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .filter(|text| text.starts_with("Bounded page history "))
            .map(|text| text.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(head_history_texts, expected_history_texts);
        let head_before = head_page.next_before.clone().expect("bounded head cursor");
        let head_revision = head_page.revision.clone();

        let older_page = load_agent_chat_page_with_revalidation_for_state(
            &final_reopened,
            agent_id.to_string(),
            Some(head_before.clone()),
            Some(head_revision.clone()),
            None,
            None,
        )
        .await
        .expect("ordinary bounded older backend page");
        let older_ids = older_page
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        assert!(older_page.next_before.is_none());
        let head_id_set = head_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert!(older_ids.iter().all(|id| !head_id_set.contains(id)));
        let older_page_pair = older_page
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .expect("older page contains the canonical commentary");
        assert_eq!(
            older_page_pair.id, canonical_b_id,
            "older page retains the post-B canonical identity"
        );
        assert_eq!(display_member_ids(older_page_pair), paired_observation_ids);
        assert_eq!(
            older_page_pair.metadata["chat_display_physical_id"].as_str(),
            Some(native.id.as_str()),
            "older page preserves the actual physical B observation"
        );
        for observation_id in &paired_observation_ids {
            assert!(older_page.aliases.iter().any(|alias| {
                alias.observation_id.as_str() == observation_id.as_str()
                    && alias.canonical_id.as_str() == canonical_b_id.as_str()
            }));
        }
        for control in &control_assistants {
            assert!(
                older_ids.contains(&control.id),
                "older page retains each distinct control"
            );
        }

        let remote_older_page =
            crate::remote::operations::remote_agent_chat_page_with_revalidation(
                &final_reopened,
                agent_id,
                Some(head_before.clone()),
                Some(head_revision.clone()),
                None,
                None,
            )
            .await
            .expect("ordinary remote older page");
        let remote_older_ids = remote_older_page
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(remote_older_ids, older_ids);
        let mut older_and_head_ids = older_ids.clone();
        older_and_head_ids.extend(head_ids.iter().cloned());
        let distinct_older_and_head = older_and_head_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(distinct_older_and_head.len(), older_and_head_ids.len());

        let overlap_page = crate::remote::operations::remote_agent_chat_page_with_revalidation(
            &final_reopened,
            agent_id,
            Some(head_before),
            Some(head_revision),
            None,
            None,
        )
        .await
        .expect("retry the opaque older page cursor");
        let overlap_ids = overlap_page
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(overlap_ids, older_ids);
        assert!(overlap_ids.iter().all(|id| older_ids.contains(id)));
        let overlap_page_pair = overlap_page
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .expect("overlap retry contains the canonical commentary");
        assert_eq!(
            overlap_page_pair.id, canonical_b_id,
            "overlap retry retains the post-B canonical identity"
        );
        let remote_assistant = remote_older_page
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .expect("remote page canonical assistant");
        assert_eq!(remote_assistant.id, canonical_b_id);
        assert_eq!(display_member_ids(remote_assistant), paired_observation_ids);
    }

    #[tokio::test]
    async fn codex_commentary_saved_off_prefix_and_deep_path_reconciles_a_then_b() {
        use std::io::Write as _;

        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().expect("isolated saved-off commentary home");
        let _home = WardianHomeGuard::set(&env_lock, home.path());
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Disabled,
            ..Default::default()
        })
        .expect("start with saved conversation logging off");

        const COMMENTARY: &str = "A source-bound commentary observation.";
        let agent_id = "commentary-off-long-path";
        let mut directory = home.path().to_path_buf();
        for component in 0..4 {
            directory.push(format!(
                "owned-source-segment-{component}-{}",
                "x".repeat(64)
            ));
        }
        std::fs::create_dir_all(&directory).expect("create deep owned source directory");
        let directory = directory
            .canonicalize()
            .expect("canonicalize owned directory");
        let log_path = directory.join("commentary-loader.jsonl");
        assert!(
            log_path.to_string_lossy().len() > 256,
            "fixture path crosses the bounded metadata header limit"
        );

        let mut lines = codex_commentary_fixture_lines(false);
        let codex_session_id = format!("{agent_id}-codex-session");
        lines.insert(
            0,
            serde_json::json!({
                "type": "session_meta",
                "payload": { "id": codex_session_id.clone() }
            })
            .to_string(),
        );
        let hidden_prefix_item = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "id": "msg-commentary-private-prefix",
                "role": "assistant",
                "phase": "commentary",
                "content": [{ "type": "output_text", "text": COMMENTARY }],
                "internal_chat_message_metadata_passthrough": {
                    "turn_id": "commentary-provider-turn",
                    "content_item_kinds": ["unknown"]
                }
            }
        });
        let prefix = format!("{}\n{}\n{}\n", lines[0], lines[1], hidden_prefix_item);
        let privacy_fence = u64::try_from(prefix.len()).expect("prefix offset fits u64");
        std::fs::write(&log_path, &prefix).expect("write source prefix while logging is off");

        let state = AppState::new();
        insert_codex_archive_test_agent(&state, agent_id, home.path(), &log_path).await;
        {
            let agents = state.agents.lock().await;
            *agents
                .get(agent_id)
                .expect("saved-off test agent")
                .current_status
                .lock()
                .expect("lock saved-off status") = "Off".into();
        }
        crate::commands::background_capture::capture_background_for_state(&state, agent_id)
            .await
            .expect("record the saved-off source boundary");
        let baseline_snapshot = agent_archive_capture_snapshot(&state, agent_id)
            .await
            .expect("saved-off capture snapshot");
        let context = conversation_archive_context_from_snapshot(&baseline_snapshot);
        let source_key = context
            .provider_source_key
            .as_deref()
            .expect("owned Codex source key");
        let baseline = state
            .conversation_archive
            .provider_log_capture_state(agent_id, source_key)
            .expect("read saved-off source state")
            .expect("saved-off pass records the source boundary");
        assert_eq!(baseline.unknown_before_offset, Some(privacy_fence));

        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("enable capture after the private prefix");
        let enabled_snapshot = agent_archive_capture_snapshot(&state, agent_id)
            .await
            .expect("enabled capture snapshot");
        record_provider_log_policy_for_snapshot(&state, &enabled_snapshot, true)
            .expect("close the saved-off boundary before new rows");

        let mut provider_log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("open owned source for A");
        writeln!(provider_log, "{}", lines[1]).expect("append A turn context");
        writeln!(provider_log, "{}", lines[2]).expect("append stream observation A");
        drop(provider_log);

        let page_a = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("ordinary bounded page reads A while capture remains off");
        let page_a_json = serde_json::to_string(&page_a).expect("serialize source page A");
        eprintln!("ISSUE1693_OFF_LONG_PATH_PAGE_A={page_a_json}");
        let a_id = page_a
            .events
            .iter()
            .find(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .map(|event| event.id.clone())
            .expect("source page exposes A before B");

        let mut provider_log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("open owned source for B");
        writeln!(provider_log, "{}", lines[3]).expect("append native response observation B");
        drop(provider_log);

        let source_snapshot = agent_archive_capture_snapshot(&state, agent_id)
            .await
            .expect("source page snapshot after B");
        let raw_seed = crate::commands::chat_recent_seed::read(&source_snapshot, None)
            .expect("read only the currently admitted bounded suffix");
        let raw_pair = raw_seed
            .events
            .iter()
            .filter(|event| {
                event.role == Some(AgentChatRole::Assistant)
                    && event.text.as_deref() == Some(COMMENTARY)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            raw_pair.len(),
            2,
            "only admitted raw A and B are in the seed"
        );
        let raw_a = raw_pair
            .iter()
            .find(|event| event.source.as_deref() == Some("event_msg"))
            .expect("A remains the native stream observation");
        let raw_b = raw_pair
            .iter()
            .find(|event| event.source.as_deref() == Some("response_item"))
            .expect("B remains the native response item");
        assert_eq!(raw_a.id, a_id);
        assert_ne!(raw_a.id, raw_b.id);
        assert_eq!(raw_a.turn_id, None);
        assert_eq!(raw_b.turn_id.as_deref(), Some("msg-commentary-native"));
        assert!(raw_pair.iter().all(|event| {
            event.metadata["chat_source_start"]
                .as_u64()
                .is_some_and(|start| start >= privacy_fence)
                && event.metadata["provider_phase"] == "commentary"
                && event.metadata["provider_turn_id"] == "commentary-provider-turn"
        }));
        assert_ne!(
            raw_a.metadata["chat_source_start"], raw_b.metadata["chat_source_start"],
            "A and B retain distinct native byte coordinates"
        );
        assert_eq!(
            raw_a.metadata["chat_source_epoch"], raw_b.metadata["chat_source_epoch"],
            "A and B remain in the same native source epoch"
        );
        assert!(raw_seed
            .events
            .iter()
            .all(|event| { event.turn_id.as_deref() != Some("msg-commentary-private-prefix") }));
        assert_eq!(raw_seed.progress, "provisional");

        let page_ab_source = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("ordinary source/provisional page after B");
        let page_ab_source_json =
            serde_json::to_string(&page_ab_source).expect("serialize provisional A/B page");
        eprintln!("ISSUE1693_OFF_LONG_PATH_PAGE_AB_SOURCE={page_ab_source_json}");

        crate::commands::background_capture::capture_background_for_state(&state, agent_id)
            .await
            .expect("publish the admitted A/B capture through the normal owner");
        let page_ab_canonical = load_agent_chat_page_with_revalidation_for_state(
            &state,
            agent_id.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("ordinary published canonical page after B");
        let page_ab_canonical_json =
            serde_json::to_string(&page_ab_canonical).expect("serialize canonical A/B page");
        eprintln!("ISSUE1693_OFF_LONG_PATH_PAGE_AB_CANONICAL={page_ab_canonical_json}");
        let after_capture = state
            .conversation_archive
            .provider_log_capture_state(agent_id, source_key)
            .expect("read source privacy state after capture")
            .expect("capture retains source state");

        let assert_one_stable_pair = |page: &wardian_core::models::chat::AgentChatPage,
                                      surface: &str| {
            let assistants = page
                .events
                .iter()
                .filter(|event| {
                    event.role == Some(AgentChatRole::Assistant)
                        && event.text.as_deref() == Some(COMMENTARY)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                assistants.len(),
                1,
                "{surface} page folds only the owned A/B pair; source={page_ab_source_json}; canonical={page_ab_canonical_json}"
            );
            let displayed = assistants[0];
            assert_eq!(displayed.id, a_id, "{surface} retains A's stable identity");
            let members = displayed.metadata["chat_display_member_ids"]
                .as_array()
                .expect("display row names its physical observations")
                .iter()
                .filter_map(|member| member.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(members.len(), 2, "{surface} retains two native members");
            assert!(members.contains(raw_a.id.as_str()));
            assert!(members.contains(raw_b.id.as_str()));
            assert!(page
                .aliases
                .iter()
                .any(|alias| { alias.observation_id == raw_b.id && alias.canonical_id == a_id }));
            assert_eq!(displayed.text.as_deref(), Some(COMMENTARY));
        };

        assert_eq!(
            page_a.events.len(),
            1,
            "initial ordinary page contains A alone"
        );
        assert_eq!(page_a.conversation_id, None);
        assert_eq!(page_a.progress, "provisional");
        assert!(page_a.events[0].metadata["chat_provisional"] == true);
        assert!(page_a.events[0].metadata["chat_identity_resolution"] == "unresolved");
        assert_eq!(
            page_a.events[0].metadata["log_path"]
                .as_str()
                .unwrap()
                .len(),
            256
        );
        assert_eq!(page_ab_source.conversation_id, None);
        assert_eq!(page_ab_source.progress, "provisional");
        assert!(page_ab_source.events.iter().all(|event| {
            event.metadata["chat_provisional"] == true
                && event.metadata["chat_identity_resolution"] == "unresolved"
        }));
        assert_eq!(
            page_ab_source.events[0].metadata["log_path"]
                .as_str()
                .unwrap()
                .len(),
            256,
            "the public header remains bounded"
        );
        assert_one_stable_pair(&page_ab_source, "source/provisional");
        assert!(page_ab_canonical.conversation_id.is_some());
        assert_eq!(after_capture.unknown_before_offset, Some(privacy_fence));
        assert_one_stable_pair(&page_ab_canonical, "published canonical");
    }

    #[tokio::test]
    async fn codex_commentary_split_capture_and_replay_preserves_unqualified_observations() {
        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().expect("isolated split commentary home");
        let _home = WardianHomeGuard::set(&env_lock, home.path());
        for reverse in [false, true] {
            let agent_id = if reverse {
                "split-commentary-reverse"
            } else {
                "split-commentary-forward"
            };
            let path = home.path().join(format!("{agent_id}.jsonl"));
            let native = codex_commentary_native_events(agent_id, &path, reverse);
            let archive = crate::state::conversation_archive::ConversationArchiveState::default();
            archive
                .append_chat_events(agent_id, &native[..1])
                .expect("capture first observation");
            archive
                .append_chat_events(agent_id, &native[1..])
                .expect("capture second observation");
            archive
                .append_chat_events(agent_id, &native)
                .expect("retry raw capture with unchanged source cursors");
            let current = merge_chat_events(Vec::new(), native.clone());
            let archived = archive
                .chat_events_for_agent(agent_id)
                .expect("query split capture");
            let projected = crate::state::conversation_archive::provenance::merge_current_capture(
                current, archived,
            )
            .expect("project split commentary capture");
            assert_unqualified_commentary_observations_preserved(
                &native,
                &projected,
                "split capture after unchanged-cursor retry",
            );
            let reopened = crate::state::conversation_archive::ConversationArchiveState::default();
            let replayed = reopened
                .chat_events_for_agent(agent_id)
                .expect("reopen split raw capture");
            let replayed = crate::state::conversation_archive::provenance::merge_current_capture(
                merge_chat_events(Vec::new(), native.clone()),
                replayed,
            )
            .expect("replay reopened split observations");
            assert_unqualified_commentary_observations_preserved(
                &native,
                &replayed,
                "reopened split capture",
            );
        }
    }

    #[tokio::test]
    async fn codex_commentary_archive_only_replay_preserves_unqualified_observations() {
        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().expect("isolated archive-only commentary home");
        let _home = WardianHomeGuard::set(&env_lock, home.path());
        for reverse in [false, true] {
            let agent_id = if reverse {
                "archive-only-reverse"
            } else {
                "archive-only-forward"
            };
            let path = home.path().join(format!("{agent_id}.jsonl"));
            let native = codex_commentary_native_events(agent_id, &path, reverse);
            let archive = crate::state::conversation_archive::ConversationArchiveState::default();
            archive
                .append_chat_events(agent_id, &native)
                .expect("capture commentary pair");
            let reopened = crate::state::conversation_archive::ConversationArchiveState::default();
            let archived = reopened
                .chat_events_for_agent(agent_id)
                .expect("archive-only replay");
            let projected = crate::state::conversation_archive::provenance::merge_current_capture(
                Vec::new(),
                archived,
            )
            .expect("project archive without a provider capture");
            assert_unqualified_commentary_observations_preserved(
                &native,
                &projected,
                "archive-only replay without native source coordinates",
            );
        }
    }

    #[tokio::test]
    async fn codex_commentary_watch_fallback_and_archive_preserve_unqualified_observations() {
        let env_lock = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().expect("isolated watch-only commentary home");
        let _home = WardianHomeGuard::set(&env_lock, home.path());
        for reverse in [false, true] {
            let agent_id = if reverse {
                "watch-only-reverse"
            } else {
                "watch-only-forward"
            };
            let path = home.path().join(format!("{agent_id}.jsonl"));
            let native = codex_commentary_native_events(agent_id, &path, reverse);
            let archive = crate::state::conversation_archive::ConversationArchiveState::default();
            archive
                .append_chat_events(agent_id, &native)
                .expect("capture raw commentary pair");
            let mut binding = crate::providers::transcript::CodexWatchBindingState::default();
            binding.set_source("commentary-native-session", &path.to_string_lossy());
            binding.observe_record(
                r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"commentary-provider-turn"}}"#,
                Some(&wardian_core::models::AgentEvent::TurnStarted {
                    turn_id: "commentary-provider-turn".into(),
                }),
            );
            let lines = codex_commentary_fixture_lines(reverse);
            let messages = lines[1..]
                .iter()
                .map(|line| {
                    binding
                        .extract_message(line)
                        .expect("extract actual watcher message shape")
                })
                .collect::<Vec<_>>();
            let transcript = WatchTranscript {
                cursor: format!("{agent_id}:0000000000000002"),
                latest_text: "A source-bound commentary observation.".into(),
                messages,
                truncated: false,
                omitted_bytes: 0,
            };
            let watch_events = transcript
                .messages
                .iter()
                .enumerate()
                .map(|(index, message)| {
                    message_event_from_transcript(
                        agent_id,
                        "codex",
                        index as u64 + 1,
                        message,
                        &transcript,
                    )
                })
                .collect::<Vec<_>>();
            let watch_observations = watch_events.clone();
            let mut current = merge_chat_events(watch_events, Vec::new());
            let mut archived = archive
                .chat_events_for_agent(agent_id)
                .expect("query durable fallback");
            // This is the ordinary Chat read path when the native log batch is
            // unavailable: coalesce watch/archive observations before replay.
            coalesce_codex_watch_observations_across(&mut current, &mut archived);
            let projected = crate::state::conversation_archive::provenance::merge_current_capture(
                current, archived,
            )
            .expect("project watcher-only fallback");
            assert_unqualified_commentary_observations_preserved(
                &native,
                &projected,
                "watch/archive fallback without native source coordinates",
            );
            for watch in &watch_observations {
                let original = native
                    .iter()
                    .find(|event| event.source == watch.source && event.turn_id == watch.turn_id)
                    .expect("watch observation has one matching raw provider type and native ID");
                let restored = projected
                    .iter()
                    .find(|event| event.id == original.id)
                    .expect("raw provider observation remains independently visible");
                let observation_ids = restored.metadata["provider_observation_ids"]
                    .as_array()
                    .expect("uniquely coalesced watcher identity remains provenance");
                assert_eq!(
                    observation_ids
                        .iter()
                        .filter(|id| id.as_str() == Some(watch.id.as_str()))
                        .count(),
                    1,
                    "watcher observation ID remains attached exactly once"
                );
            }
        }
    }

    #[test]
    fn codex_commentary_projection_preserves_distinct_native_ids_and_ambiguous_mirrors() {
        let directory = tempfile::tempdir().expect("commentary ambiguity fixture");
        let path = directory.path().join("commentary.jsonl");
        let mut lines = codex_commentary_fixture_lines(false);
        let mut other: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
        other["payload"]["id"] = serde_json::json!("msg-commentary-other-native");
        lines.push(other.to_string());
        let mut events = normalize_chat_lines("commentary-agent", "codex", &lines);
        decorate_forward_provider_log_events(&mut events, "codex", &path);
        assert_eq!(events.len(), 3);
        let identified = events
            .iter()
            .filter(|event| event.turn_id.is_some())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            merge_chat_events(Vec::new(), identified.clone()).len(),
            2,
            "equal text never joins distinct identified native messages"
        );
        let mut watch = identified[1].clone();
        watch.id = "watch-distinct-native-message".into();
        watch.metadata["transcript_cursor"] = serde_json::json!("watch:1");
        assert_eq!(
            merge_chat_events(vec![watch], vec![identified[0].clone()]).len(),
            2,
            "a distinct watcher native ID must survive equal provider text"
        );
        let mut archived = identified[0].clone();
        archived.metadata["conversation_archive_id"] = serde_json::json!("archived-native");
        archived.metadata["provider_observation_ids"] =
            serde_json::json!([archived.id, identified[1].id]);
        assert_eq!(
            merge_chat_events(Vec::new(), vec![archived, identified[1].clone()]).len(),
            2,
            "a historical alias cannot join distinct identified native messages"
        );
        for reverse in [false, true] {
            let mut ordered = events.clone();
            if reverse {
                ordered.reverse();
            }
            let projected = merge_chat_events(Vec::new(), ordered);
            assert_eq!(
                projected.len(),
                3,
                "an identity-less mirror with two native candidates remains ambiguous"
            );
        }
    }

    #[test]
    fn codex_commentary_projection_preserves_conflicting_phase_and_native_bindings() {
        let directory = tempfile::tempdir().expect("commentary contradiction fixture");
        let path = directory.path().join("commentary.jsonl");
        let base = codex_commentary_native_events("commentary-agent", &path, false);
        let mut cross_phase_lines = codex_commentary_fixture_lines(false);
        let mut completed: serde_json::Value = serde_json::from_str(&cross_phase_lines[2]).unwrap();
        completed["payload"]["phase"] = serde_json::json!("final_answer");
        cross_phase_lines[2] = completed.to_string();
        let mut cross_phase = normalize_chat_lines("commentary-agent", "codex", &cross_phase_lines);
        decorate_forward_provider_log_events(&mut cross_phase, "codex", &path);
        let mut cases = vec![("commentary versus final", cross_phase)];

        let mut different_turn = base.clone();
        different_turn[1].metadata["provider_turn_id"] =
            serde_json::json!("different-provider-turn");
        cases.push(("different provider turn", different_turn));
        let mut different_native_session = base.clone();
        different_native_session[0].metadata["provider_session_id"] =
            serde_json::json!("native-session-one");
        different_native_session[1].metadata["provider_session_id"] =
            serde_json::json!("native-session-two");
        cases.push(("conflicting native session", different_native_session));
        let mut different_source = base.clone();
        different_source[1].metadata["log_path"] = serde_json::json!("other-native-source.jsonl");
        cases.push(("different native source", different_source));
        let mut different_agent = base.clone();
        different_agent[1].session_id = "other-wardian-agent".into();
        cases.push(("different Wardian session", different_agent));
        for (reason, events) in cases {
            assert_eq!(merge_chat_events(Vec::new(), events).len(), 2, "{reason}");
        }
    }

    #[test]
    fn codex_commentary_projection_preserves_unbound_or_phase_absent_observations() {
        let directory = tempfile::tempdir().expect("unbound commentary fixture");
        let path = directory.path().join("commentary.jsonl");
        let mut unbound_lines = codex_commentary_fixture_lines(false);
        unbound_lines.remove(0);
        let mut unbound = normalize_chat_lines("commentary-agent", "codex", &unbound_lines);
        decorate_forward_provider_log_events(&mut unbound, "codex", &path);
        assert!(unbound[0].metadata["provider_turn_id"].is_null());
        assert_eq!(
            unbound[1].metadata["provider_turn_id"],
            "commentary-provider-turn"
        );
        assert_eq!(
            merge_chat_events(Vec::new(), unbound).len(),
            2,
            "an unbound stream row cannot join by equal text to a turn-bound response"
        );

        let mut phase_absent_lines = codex_commentary_fixture_lines(false);
        let mut stream: serde_json::Value = serde_json::from_str(&phase_absent_lines[1]).unwrap();
        stream["payload"].as_object_mut().unwrap().remove("phase");
        phase_absent_lines[1] = stream.to_string();
        let mut phase_absent =
            normalize_chat_lines("commentary-agent", "codex", &phase_absent_lines);
        decorate_forward_provider_log_events(&mut phase_absent, "codex", &path);
        assert!(
            phase_absent[0].metadata["provider_phase"].is_null(),
            "absence remains absence"
        );
        assert_eq!(
            merge_chat_events(Vec::new(), phase_absent).len(),
            2,
            "a missing phase cannot prove commentary/final equivalence"
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
    fn codex_user_response_item_survives_merge_as_a_request_turn() {
        let provider_events = normalize_chat_lines(
            "agent-1",
            "codex",
            [
                r#"{"type":"response_item","payload":{"type":"message","id":"native-request","role":"user","content":[{"type":"input_text","text":"Run the archive check."}],"internal_chat_message_metadata_passthrough":{"turn_id":"native-turn","content_item_kinds":["user.text"]}}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"Run the archive check."}}"#,
                r#"{"type":"response_item","payload":{"type":"message","id":"native-answer","role":"assistant","content":[{"type":"output_text","text":"Archive checked."}]}}"#,
            ],
        );
        let chat_events = merge_chat_events(Vec::new(), provider_events);
        let request = chat_events
            .iter()
            .find(|event| event.text.as_deref() == Some("Run the archive check."))
            .expect("merged request");

        assert_eq!(chat_events.len(), 2);
        assert_eq!(request.source.as_deref(), Some("response_item"));
        assert_eq!(request.role, Some(AgentChatRole::User));
        assert_eq!(request.turn_id.as_deref(), Some("native-request"));
        assert_eq!(request.metadata["input_origin"], "human_input");
        assert_eq!(request.metadata["input_purpose"], "request");
        assert_eq!(request.metadata["provider_turn_id"], "native-turn");
        assert_eq!(request.metadata["request_root_id"], "native-request");

        let records = chat_events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                crate::state::conversation_archive::narrative_from_chat_event(
                    event,
                    index as u64 + 1,
                )
            })
            .collect::<Vec<_>>();
        let turns = crate::state::conversation_archive::derive_turn_records(
            "conversation-1",
            &records,
            &chat_events,
            &[],
            false,
        );

        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].request.kind, "user_request",
            "an explicit user response_item must remain a request after mirror merging"
        );
        assert_eq!(
            turns[0].status,
            wardian_core::conversations::ConversationTurnStatus::Responded
        );
    }

    #[test]
    fn canonicalizes_codex_user_mirrors_by_native_turn_and_preserves_repeated_turns() {
        let mut lines = include_str!("../providers/fixtures/codex-user-input-mirror.jsonl")
            .lines()
            .collect::<Vec<_>>();
        lines.extend([
            r#"{"type":"turn_context","payload":{"turn_id":"provider-turn-b"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"message-b","role":"user","content":[{"type":"input_text","text":"Inspect the archive."}],"internal_chat_message_metadata_passthrough":{"turn_id":"provider-turn-b","content_item_kinds":["user.text"]}}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","id":"msg:provider-turn-b","client_id":"client-b","message":"Inspect the archive."}}"#,
        ]);
        let mut provider_events = normalize_chat_lines("agent-1", "codex", lines);
        for event in &mut provider_events {
            event.metadata["provider_log"] = serde_json::json!(true);
            event.metadata["provider_session_id"] = serde_json::json!("provider-session");
            event.metadata["log_path"] = serde_json::json!("<provider-log>");
            if event.role == Some(AgentChatRole::User)
                && event.source.as_deref() == Some("event_msg")
            {
                event.metadata["request_root_id"] = serde_json::json!(format!(
                    "msg:{}",
                    event.metadata["provider_turn_id"].as_str().unwrap()
                ));
            }
        }
        let raw_user_ids = provider_events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::User))
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        let raw_user_roots = provider_events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::User))
            .map(|event| {
                event.metadata["request_root_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();

        let projected = merge_chat_events(Vec::new(), provider_events);
        let users = projected
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::User))
            .collect::<Vec<_>>();

        assert_eq!(raw_user_ids.len(), 4);
        assert_ne!(raw_user_roots[0], raw_user_roots[1]);
        assert_ne!(raw_user_roots[2], raw_user_roots[3]);
        assert_ne!(raw_user_roots[0], raw_user_roots[2]);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].text, users[1].text);
        assert_ne!(
            users[0].metadata["request_root_id"],
            users[1].metadata["request_root_id"]
        );
        assert_eq!(
            users
                .iter()
                .map(|event| event.metadata["provider_observation_ids"]
                    .as_array()
                    .expect("observation IDs")
                    .len())
                .collect::<Vec<_>>(),
            vec![2, 2]
        );
        assert_eq!(
            users
                .iter()
                .map(|event| event.metadata["provider_turn_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["provider-turn-a", "provider-turn-b"]
        );
        assert_eq!(
            users
                .iter()
                .map(
                    |event| event.metadata["provider_mirror_request_root_ids"][0]
                        .as_str()
                        .unwrap()
                )
                .collect::<Vec<_>>(),
            vec!["msg:provider-turn-a", "msg:provider-turn-b"]
        );
        assert!(users.iter().all(|event| {
            event.metadata["provider_observation_ids"]
                .as_array()
                .unwrap()
                .iter()
                .all(|id| {
                    id.as_str()
                        .is_some_and(|id| raw_user_ids.iter().any(|raw_id| raw_id == id))
                })
        }));
    }

    #[test]
    fn does_not_canonicalize_codex_user_rows_with_changed_text_in_one_turn() {
        let mut events = normalize_chat_lines(
            "agent-1",
            "codex",
            [
                r#"{"type":"turn_context","payload":{"turn_id":"provider-turn-a"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","id":"native-request-a","role":"user","content":[{"type":"input_text","text":"Inspect the archive."}],"internal_chat_message_metadata_passthrough":{"turn_id":"provider-turn-a","content_item_kinds":["user.text"]}}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"Inspect another archive."}}"#,
            ],
        );
        for event in &mut events {
            event.metadata["provider_log"] = serde_json::json!(true);
            event.metadata["provider_session_id"] = serde_json::json!("provider-session");
            event.metadata["log_path"] = serde_json::json!("<codex-log>");
        }

        let projected = merge_chat_events(Vec::new(), events);
        let users = projected
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::User))
            .collect::<Vec<_>>();

        assert_eq!(users.len(), 2);
        assert_ne!(users[0].text, users[1].text);
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
        // Field 5.3 is metadata.source (4 = USER_EXPLICIT, 2 = MODEL),
        // field 19.2 is the user message, and 20.1 is the planner response.
        // These source values match the paired SQLite/JSONL QA recording.
        let legacy_user = vec![0x9a, 0x01, 0x05, 0x12, 0x03, b'h', b'i', b'!'];
        let mut user = vec![0x2a, 0x02, 0x18, 0x04];
        user.extend(&legacy_user);
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
        for (step, source) in [(2, 2), (3, 99)] {
            let mut payload = vec![0x2a, 0x02, 0x18, source];
            payload.extend(&legacy_user);
            connection
                .execute(
                    "INSERT INTO steps VALUES (?1, 14, ?2)",
                    rusqlite::params![step, payload],
                )
                .expect("insert non-user source with user-shaped text");
        }
        for (step, kind, payload) in [
            (4, 14, legacy_user.clone()),
            (5, 999, user.clone()),
            (6, 14, vec![0x2a, 0x02, 0x18, 0x04]),
            (7, 14, user),
        ] {
            connection
                .execute(
                    "INSERT INTO steps VALUES (?1, ?2, ?3)",
                    rusqlite::params![step, kind, payload],
                )
                .expect("insert compatibility and repeated-request fixtures");
        }

        let events = load_antigravity_database_chat_events("agent-1", "antigravity", &database);

        assert_eq!(events.len(), 6);
        assert_eq!(events[0].role, Some(AgentChatRole::User));
        assert_eq!(events[0].text.as_deref(), Some("hi!"));
        assert_eq!(events[0].metadata["input_origin"], "human_input");
        assert_eq!(events[0].metadata["input_purpose"], "request");
        assert_eq!(events[0].metadata["context_observation"], "unreported");
        assert_eq!(events[0].metadata["request_root_id"], events[0].id);
        assert_eq!(events[0].metadata["provider_step_source"], 4);
        assert_eq!(events[1].role, Some(AgentChatRole::Assistant));
        assert_eq!(events[1].text.as_deref(), Some("ok!"));
        assert!(events[1].metadata.get("input_origin").is_none());
        assert!(events[1].metadata.get("request_root_id").is_none());
        for (event, source) in [(&events[2], 2), (&events[3], 99)] {
            assert_eq!(event.role, Some(AgentChatRole::System));
            let mut legacy = event.clone();
            legacy.role = Some(AgentChatRole::User);
            assert_eq!(event.id, stable_provider_log_event_id(&legacy, &database));
            assert_eq!(event.text.as_deref(), Some("hi!"));
            assert_eq!(event.metadata["provider_step_source"], source);
            assert_eq!(event.metadata["input_origin"], "provider_internal");
            assert_eq!(event.metadata["input_purpose"], "internal");
            assert!(event.metadata.get("request_root_id").is_none());
        }
        for event in [&events[4], &events[5]] {
            assert_eq!(event.metadata["input_origin"], "human_input");
            assert_eq!(event.metadata["input_purpose"], "request");
            assert_eq!(event.metadata["request_root_id"], event.id);
            assert_ne!(
                event.metadata["request_root_id"],
                events[0].metadata["request_root_id"]
            );
        }
        let replay = load_antigravity_database_chat_events("agent-1", "antigravity", &database);
        for (original, replayed) in events.iter().zip(&replay) {
            assert_eq!(original.id, replayed.id);
            assert_eq!(original.metadata, replayed.metadata);
        }
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
        assert_eq!(
            chat_events[2].metadata["source_path"],
            db_path.to_string_lossy().as_ref()
        );

        conn.execute(
            "INSERT INTO message VALUES ('msg-later', 'ses_test', 7, 7, '{\"role\":\"assistant\"}')",
            [],
        )
        .expect("append message");
        conn.execute(
            "INSERT INTO part VALUES ('part-later', 'msg-later', 'ses_test', 8, 8, '{\"type\":\"text\",\"text\":\"A later reply\"}')",
            [],
        )
        .expect("append part");

        let refreshed = load_opencode_db_chat_events_from_db(&db_path, "agent-1", "ses_test")
            .expect("refresh db");
        assert_eq!(refreshed.len(), 4);
        assert_eq!(refreshed[3].text.as_deref(), Some("A later reply"));
    }

    #[test]
    fn opencode_session_id_prefers_real_resume_session() {
        assert_eq!(
            opencode_session_id("wardian-uuid", Some("ses_real"), None).as_deref(),
            Some("ses_real")
        );
        assert_eq!(
            opencode_session_id("ses_from_agent", None, None).as_deref(),
            Some("ses_from_agent")
        );
        assert_eq!(
            opencode_session_id("wardian-uuid", Some("stale-uuid"), Some("ses_fresh")),
            Some("ses_fresh".to_string())
        );
        assert_eq!(
            opencode_session_id("wardian-uuid", Some("stale-uuid"), Some("fresh-uuid")),
            None
        );
        assert_eq!(
            opencode_session_id("wardian-uuid", None, Some("ses_fresh")).as_deref(),
            Some("ses_fresh")
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
