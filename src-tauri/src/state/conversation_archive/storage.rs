use std::{
    collections::{HashMap, HashSet},
    io,
    sync::{Arc, Mutex},
};

use uuid::Uuid;
use wardian_core::conversations::{
    append_index_upsert, materialize_text_payload, read_jsonl_records, read_latest_index_entries,
    write_json_atomic, write_jsonl_atomic, ConversationBoundaryReason, ConversationFormatVersions,
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationRecordKind, ConversationSourceRecord,
    ConversationStatus, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::AgentChatEvent;
use wardian_core::paths::{agent_conversation_dir, agent_conversations_dir, agents_dir};

use super::records::{current_rfc3339_millis, metadata_string};
use super::turns::{
    apply_archive_summary_to_manifest, archive_summary, derive_turn_records_with_context,
};
use super::{ActiveConversationHandle, ConversationArchiveContext, ConversationCaptureState};

pub(super) fn lock_active(
    active: &Mutex<HashMap<String, ActiveConversationHandle>>,
) -> io::Result<std::sync::MutexGuard<'_, HashMap<String, ActiveConversationHandle>>> {
    active
        .lock()
        .map_err(|_| io::Error::other("conversation archive state lock poisoned"))
}

pub(super) fn agent_lock_for(
    locks: &Mutex<HashMap<String, Arc<Mutex<()>>>>,
    agent_id: &str,
) -> io::Result<Arc<Mutex<()>>> {
    let mut locks = locks
        .lock()
        .map_err(|_| io::Error::other("conversation archive lock map poisoned"))?;
    Ok(locks
        .entry(agent_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

pub(super) fn lock_agent_archive(
    lock: &Arc<Mutex<()>>,
) -> io::Result<std::sync::MutexGuard<'_, ()>> {
    lock.lock()
        .map_err(|_| io::Error::other("conversation archive agent lock poisoned"))
}

pub(super) fn conversation_dir(
    agent_id: &str,
    conversation_id: &str,
) -> io::Result<std::path::PathBuf> {
    agent_conversation_dir(agent_id, conversation_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe conversation path"))
}

pub(super) fn index_path(agent_id: &str) -> io::Result<std::path::PathBuf> {
    agent_conversations_dir(agent_id)
        .map(|dir| dir.join("index.jsonl"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe agent path"))
}

fn capture_state_path(agent_id: &str) -> io::Result<std::path::PathBuf> {
    agent_conversations_dir(agent_id)
        .and_then(|dir| {
            dir.parent()
                .map(|agent_dir| agent_dir.join("conversation-capture.json"))
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe agent path"))
}

pub(super) fn read_capture_state(agent_id: &str) -> io::Result<ConversationCaptureState> {
    let path = capture_state_path(agent_id)?;
    if !path.exists() {
        return Ok(ConversationCaptureState::default());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(io::Error::other)
}

pub(super) fn write_capture_state(
    agent_id: &str,
    state: &ConversationCaptureState,
) -> io::Result<()> {
    write_json_atomic(&capture_state_path(agent_id)?, state)
}

pub(super) fn read_agent_index(agent_id: &str) -> io::Result<Vec<ConversationIndexEntry>> {
    read_latest_index_entries(&index_path(agent_id)?)
}

pub(super) fn read_all_agent_indexes() -> io::Result<Vec<ConversationIndexEntry>> {
    let Some(agents_dir) = agents_dir() else {
        return Ok(Vec::new());
    };
    if !agents_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(agents_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(agent_id) = entry.file_name().to_str().map(ToString::to_string) else {
            continue;
        };
        entries.extend(read_agent_index(&agent_id)?);
    }
    sort_conversation_entries(&mut entries);
    Ok(entries)
}

fn sort_conversation_entries(entries: &mut [ConversationIndexEntry]) {
    entries.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then_with(|| left.conversation_id.cmp(&right.conversation_id))
    });
}

pub(super) fn new_conversation_id(agent_id: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
    let nonce = Uuid::new_v4().simple();
    format!("conv_{timestamp}_{nonce}_{}", safe_agent_suffix(agent_id))
}

fn safe_agent_suffix(agent_id: &str) -> String {
    let suffix = agent_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(48)
        .collect::<String>();
    if suffix.is_empty() {
        "agent".to_string()
    } else {
        suffix
    }
}

pub(super) fn provider_from_events(events: &[AgentChatEvent]) -> Option<String> {
    events
        .iter()
        .map(|event| event.provider.trim())
        .find(|provider| !provider.is_empty())
        .map(ToString::to_string)
}

pub(super) fn provider_source_key_from_events(events: &[AgentChatEvent]) -> Option<String> {
    events.iter().find_map(|event| {
        let provider = event.provider.trim();
        if provider.is_empty() {
            return None;
        }
        provider_session_id_from_event(event)
            .map(|session_id| format!("{provider}:session:{session_id}"))
            .or_else(|| {
                source_path_from_event(event).map(|path| format!("{provider}:source:{path}"))
            })
    })
}

pub(super) fn provider_session_ids_from_events(events: &[AgentChatEvent]) -> Vec<String> {
    let mut seen = HashSet::new();
    events
        .iter()
        .filter_map(provider_session_id_from_event)
        .filter(|session_id| seen.insert(session_id.clone()))
        .collect()
}

fn provider_session_id_from_event(event: &AgentChatEvent) -> Option<String> {
    metadata_string(&event.metadata, "opencode_session_id")
        .or_else(|| metadata_string(&event.metadata, "provider_session_id"))
}

fn source_path_from_event(event: &AgentChatEvent) -> Option<String> {
    event
        .path
        .clone()
        .or_else(|| metadata_string(&event.metadata, "source_path"))
        .or_else(|| metadata_string(&event.metadata, "path"))
        .or_else(|| metadata_string(&event.metadata, "log_path"))
}

pub(super) fn active_handle_for_context(
    active: &Mutex<HashMap<String, ActiveConversationHandle>>,
    context: &ConversationArchiveContext,
    provider_source_key: Option<String>,
) -> io::Result<ActiveConversationHandle> {
    let existing = {
        let active = lock_active(active)?;
        active.get(&context.agent_id).cloned()
    };

    if let Some(existing) = existing {
        match (
            existing.provider_source_key.as_deref(),
            provider_source_key.as_deref(),
        ) {
            (Some(existing_key), Some(next_key)) if existing_key != next_key => {
                {
                    let mut active = lock_active(active)?;
                    active.remove(&context.agent_id);
                }
                close_conversation_handle(
                    &context.agent_id,
                    &existing,
                    ConversationBoundaryReason::ProviderSourceChanged,
                )?;
            }
            (None, Some(_)) => {
                return Ok(ActiveConversationHandle {
                    provider_source_key,
                    ..existing
                });
            }
            _ => {
                return Ok(existing);
            }
        }
    }

    if let Some(hydrated) = hydrate_open_handle(context, provider_source_key.as_deref())? {
        return Ok(hydrated);
    }

    Ok(ActiveConversationHandle {
        conversation_id: new_conversation_id(&context.agent_id),
        next_seq: 1,
        provider_source_key,
    })
}

fn hydrate_open_handle(
    context: &ConversationArchiveContext,
    provider_source_key: Option<&str>,
) -> io::Result<Option<ActiveConversationHandle>> {
    for entry in read_agent_index(&context.agent_id)? {
        if entry.status != ConversationStatus::Open {
            continue;
        }
        let conversation_dir = conversation_dir(&entry.agent_id, &entry.conversation_id)?;
        let Some(manifest) = read_manifest(&conversation_dir.join("manifest.json"))? else {
            continue;
        };
        if manifest.status != ConversationStatus::Open {
            continue;
        }
        if context.provider != "unknown" && manifest.provider != context.provider {
            continue;
        }
        let keys_are_compatible =
            match (manifest.provider_source_key.as_deref(), provider_source_key) {
                (Some(existing_key), Some(next_key)) if existing_key != next_key => {
                    close_conversation_dir(
                        &entry.agent_id,
                        &entry.conversation_id,
                        &conversation_dir,
                        ConversationBoundaryReason::ProviderSourceChanged,
                    )?;
                    false
                }
                (Some(_), Some(_)) | (None, Some(_)) | (Some(_), None) | (None, None) => true,
            };
        if !keys_are_compatible {
            continue;
        }
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_dir.join("conversation.jsonl"))?;
        let next_seq = records
            .iter()
            .map(|record| record.seq)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        return Ok(Some(ActiveConversationHandle {
            conversation_id: manifest.conversation_id,
            next_seq,
            provider_source_key: provider_source_key
                .map(ToString::to_string)
                .or(manifest.provider_source_key),
        }));
    }
    Ok(None)
}

pub(super) fn effective_context_for_handle(
    context: &ConversationArchiveContext,
    handle: &ActiveConversationHandle,
    conversation_dir: &std::path::Path,
) -> io::Result<ConversationArchiveContext> {
    let existing_manifest = read_manifest(&conversation_dir.join("manifest.json"))?;
    let mut effective = context.clone();

    if effective.provider_source_key.is_none() {
        effective.provider_source_key = handle.provider_source_key.clone().or_else(|| {
            existing_manifest
                .as_ref()
                .and_then(|manifest| manifest.provider_source_key.clone())
        });
    }

    if let Some(manifest) = existing_manifest.as_ref() {
        if effective.provider == "unknown" {
            effective.provider = manifest.provider.clone();
        }
        if effective.agent_name.trim().is_empty() || effective.agent_name == effective.agent_id {
            effective.agent_name = manifest.agent_name.clone();
        }
        if effective.agent_class.trim().is_empty() {
            effective.agent_class = manifest.agent_class.clone();
        }
        if effective.workspace.trim().is_empty() {
            effective.workspace = manifest.workspace.clone();
        }
        if effective.provider_session_ids.is_empty() {
            effective.provider_session_ids = manifest.provider_session_ids.clone();
        }
    }

    Ok(effective)
}

fn close_conversation_handle(
    agent_id: &str,
    handle: &ActiveConversationHandle,
    reason: ConversationBoundaryReason,
) -> io::Result<()> {
    let conversation_dir = conversation_dir(agent_id, &handle.conversation_id)?;
    close_conversation_dir(agent_id, &handle.conversation_id, &conversation_dir, reason)
}

pub(super) fn close_conversation_dir(
    agent_id: &str,
    conversation_id: &str,
    conversation_dir: &std::path::Path,
    reason: ConversationBoundaryReason,
) -> io::Result<()> {
    let conversation_path = conversation_dir.join("conversation.jsonl");
    let records: Vec<ConversationNarrativeRecord> = read_jsonl_records(&conversation_path)?;
    let events: Vec<AgentChatEvent> = read_jsonl_records(&conversation_dir.join("events.jsonl"))?;
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_dir.join("sources.jsonl"))?;
    let now = current_rfc3339_millis();
    let mut manifest =
        read_manifest(&conversation_dir.join("manifest.json"))?.unwrap_or_else(|| {
            let created_at = records
                .first()
                .map(|record| record.at.clone())
                .unwrap_or_else(|| now.clone());
            let context = ConversationArchiveContext::for_agent_id(agent_id, "unknown");
            open_manifest(&context, conversation_id, created_at, now.clone())
        });
    let turns = derive_turn_records_with_context(
        conversation_id,
        &records,
        &events,
        &sources,
        false,
        Some(&manifest.provider),
        &manifest.provider_session_ids,
    );
    write_jsonl_atomic(&conversation_dir.join("turns.jsonl"), &turns)?;
    let summary = archive_summary(&records, &turns, &sources);
    manifest.updated_at = now.clone();
    manifest.closed_at = Some(now.clone());
    manifest.status = status_for_boundary_reason(reason);
    manifest.boundary_reason = reason;
    apply_archive_summary_to_manifest(&mut manifest, &summary);
    write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
    append_index_upsert(
        &index_path(agent_id)?,
        &index_entry_from_manifest(
            &manifest,
            Some(now),
            records.first().and_then(excerpt_from_record),
            records.last().and_then(excerpt_from_record),
            records.len() as u64,
            artifact_count_for_records(records.iter()),
        ),
    )
}

pub(super) fn read_manifest(path: &std::path::Path) -> io::Result<Option<ConversationManifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(io::Error::other)
}

pub(super) fn open_manifest(
    context: &ConversationArchiveContext,
    conversation_id: &str,
    created_at: String,
    updated_at: String,
) -> ConversationManifest {
    ConversationManifest {
        schema: CONVERSATION_SCHEMA,
        conversation_id: conversation_id.to_string(),
        agent_id: context.agent_id.clone(),
        agent_name: context.agent_name.clone(),
        agent_class: context.agent_class.clone(),
        workspace: context.workspace.clone(),
        provider: context.provider.clone(),
        provider_session_ids: context.provider_session_ids.clone(),
        provider_source_key: context.provider_source_key.clone(),
        provider_native_refs: Vec::new(),
        effective_logging: ConversationLoggingSetting::Enabled,
        created_at,
        updated_at,
        closed_at: None,
        status: ConversationStatus::Open,
        boundary_reason: ConversationBoundaryReason::Spawn,
        format_versions: ConversationFormatVersions::default(),
        record_count: 0,
        turn_count: 0,
        has_turns: false,
        lifecycle_only: false,
    }
}

pub(super) fn index_entry_from_manifest(
    manifest: &ConversationManifest,
    ended_at: Option<String>,
    first_prompt_excerpt: Option<String>,
    last_record_excerpt: Option<String>,
    record_count: u64,
    artifact_count: u64,
) -> ConversationIndexEntry {
    ConversationIndexEntry {
        schema: CONVERSATION_SCHEMA,
        conversation_id: manifest.conversation_id.clone(),
        agent_id: manifest.agent_id.clone(),
        agent_name: manifest.agent_name.clone(),
        agent_class: manifest.agent_class.clone(),
        workspace: manifest.workspace.clone(),
        provider: manifest.provider.clone(),
        provider_session_ids: manifest.provider_session_ids.clone(),
        started_at: manifest.created_at.clone(),
        ended_at,
        status: manifest.status,
        boundary_reason: manifest.boundary_reason,
        first_prompt_excerpt,
        last_record_excerpt,
        record_count,
        turn_count: manifest.turn_count,
        has_turns: manifest.has_turns,
        lifecycle_only: manifest.lifecycle_only,
        artifact_count,
        path: format!(
            "agents/{}/conversations/{}",
            manifest.agent_id, manifest.conversation_id
        ),
    }
}

pub(super) fn materialize_record_text(
    conversation_dir: &std::path::Path,
    record: &mut ConversationNarrativeRecord,
) -> io::Result<()> {
    let Some(text) = record.text.as_deref() else {
        return Ok(());
    };
    let payload = materialize_text_payload(
        &conversation_dir.join("artifacts"),
        &artifact_stem_for_record(record),
        text,
    )?;
    record.text = payload.text;
    record.excerpt = payload.excerpt;
    record.artifact_refs = payload.artifact_refs;
    Ok(())
}

pub(super) fn event_record_for_jsonl(
    event: &AgentChatEvent,
    record: &ConversationNarrativeRecord,
) -> AgentChatEvent {
    if event.text.is_none() || record.text.is_some() || record.artifact_refs.is_empty() {
        return event.clone();
    }

    let mut event = event.clone();
    event.text = None;
    let metadata = event
        .metadata
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .chain([
            (
                "text_excerpt".to_string(),
                record
                    .excerpt
                    .clone()
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "text_artifact_refs".to_string(),
                serde_json::Value::Array(
                    record
                        .artifact_refs
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            ),
        ])
        .collect();
    event.metadata = serde_json::Value::Object(metadata);
    event
}

fn artifact_stem_for_record(record: &ConversationNarrativeRecord) -> String {
    if let Some(event_ref) = record.event_refs.first() {
        return event_ref.clone();
    }

    match (record.kind, record.role.as_deref()) {
        (ConversationRecordKind::Message, Some("user")) => {
            format!("delivered-input-{}", record.seq)
        }
        (ConversationRecordKind::Lifecycle, _) => format!("lifecycle-{}", record.seq),
        _ => format!("record-{}", record.seq),
    }
}

pub(super) fn artifact_count_for_records<'a>(
    records: impl Iterator<Item = &'a ConversationNarrativeRecord>,
) -> u64 {
    records
        .map(|record| record.artifact_refs.len() as u64)
        .sum()
}

pub(super) fn excerpt_from_record(record: &ConversationNarrativeRecord) -> Option<String> {
    record
        .text
        .as_deref()
        .or(record.excerpt.as_deref())
        .or(record.summary.as_deref())
        .map(bounded_excerpt)
}

fn bounded_excerpt(text: &str) -> String {
    const LIMIT: usize = 240;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= LIMIT {
        return normalized;
    }
    let mut end = 0;
    for (index, ch) in normalized.char_indices() {
        let next = index + ch.len_utf8();
        if next > LIMIT {
            break;
        }
        end = next;
    }
    normalized[..end].to_string()
}

fn status_for_boundary_reason(reason: ConversationBoundaryReason) -> ConversationStatus {
    match reason {
        ConversationBoundaryReason::ProviderSourceChanged
        | ConversationBoundaryReason::Shutdown => ConversationStatus::Interrupted,
        ConversationBoundaryReason::Spawn
        | ConversationBoundaryReason::Clear
        | ConversationBoundaryReason::WorktreeSwitch
        | ConversationBoundaryReason::LoggingEnabled => ConversationStatus::Closed,
    }
}
