#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, read_jsonl_records, read_jsonl_records_resilient,
    write_json_atomic, write_jsonl_atomic, AgentConversationLoggingSetting,
    ConversationBoundaryReason, ConversationIndexEntry, ConversationLoggingSetting,
    ConversationManifest, ConversationNarrativeRecord, ConversationRecordKind,
    ConversationSourceRecord, ConversationSpeakerType, ConversationTurnRecord,
};
use wardian_core::models::chat::AgentChatEvent;

pub(crate) mod provenance;
mod records;
mod repair;
mod storage;
#[cfg(test)]
mod tests;
mod turns;

use records::{
    current_rfc3339_millis, generated_event_from_record, generated_sources_from_record,
    matching_delivered_input_record_index, record_kind_from_chat_event_kind,
    source_record_from_chat_event,
};
pub use records::{lifecycle_record, narrative_from_chat_event, narrative_from_delivered_input};
use repair::{
    append_source_if_needed, coalesce_batch_observations, is_bound_native_delivery,
    matching_event_index, matching_record_index, publish_recovered_observations,
    rebuild_derived_projections, recover_unlinked_observations, PendingChatPublication,
};
#[cfg(test)]
use storage::new_conversation_id;
use storage::{
    active_handle_for_context, agent_lock_for, artifact_count_for_records, close_conversation_dir,
    conversation_dir, effective_context_for_handle, event_record_for_jsonl, excerpt_from_record,
    index_entry_from_manifest, index_path, lock_active, lock_agent_archive,
    materialize_record_text, open_manifest, provider_from_events, provider_session_ids_from_events,
    provider_source_key_from_events, read_agent_index, read_all_agent_indexes, read_capture_state,
    read_manifest, write_capture_state,
};
#[cfg(test)]
pub(crate) use turns::derive_turn_records;
use turns::{apply_archive_summary_to_manifest, archive_summary, derive_turn_records_with_context};

#[derive(Debug, Default)]
pub struct ConversationArchiveState {
    #[allow(dead_code)]
    active: Mutex<HashMap<String, ActiveConversationHandle>>,
    live_started_at: Mutex<HashMap<String, String>>,
    agent_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    #[cfg(test)]
    fail_next_rollover_after_close: AtomicBool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConversationHandle {
    pub conversation_id: String,
    pub next_seq: u64,
    pub provider_source_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationArchiveContext {
    pub agent_id: String,
    pub agent_name: String,
    pub agent_class: String,
    pub workspace: String,
    pub provider: String,
    pub provider_session_ids: Vec<String>,
    pub provider_source_key: Option<String>,
}

impl ConversationArchiveContext {
    pub fn for_agent_id(agent_id: &str, provider: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            agent_name: agent_id.to_string(),
            agent_class: String::new(),
            workspace: String::new(),
            provider: provider.to_string(),
            provider_session_ids: Vec::new(),
            provider_source_key: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct ConversationCaptureState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_events_at_or_before: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skip_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skip_event_scopes: Vec<ConversationCaptureEventScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    provider_log_sources: Vec<crate::commands::provider_log_acquisition::ProviderLogCaptureState>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct ConversationCaptureEventScope {
    provider_source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_events_at_or_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disabled_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disabled_until: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    event_ids: Vec<String>,
}

impl ConversationCaptureState {
    fn should_skip_event(&self, event: &AgentChatEvent, provider_source_key: Option<&str>) -> bool {
        let event_ids = event_identity_ids(event);
        let legacy_unscoped_match = provider_source_key.is_none()
            && event_ids
                .iter()
                .any(|event_id| self.skip_event_ids.iter().any(|id| id == event_id));
        let scoped_match = self.skip_event_scopes.iter().any(|scope| {
            if scope.provider_source_key.as_deref() != provider_source_key {
                return false;
            }
            if event_ids
                .iter()
                .any(|event_id| scope.event_ids.iter().any(|id| id == event_id))
            {
                return true;
            }
            let is_canonical_opencode_db_event = event.source.as_deref() == Some("opencode_db")
                && event
                    .metadata
                    .get("part_id")
                    .and_then(|value| value.as_str())
                    .is_some();
            // A closed policy interval classifies canonical DB rows by their
            // own creation time. The byte-log cutoff remains the fallback for
            // legacy state and non-canonical provider observations.
            let cutoff_match = scope
                .skip_events_at_or_before
                .as_deref()
                .zip(event.created_at.as_deref())
                .is_some_and(|(cutoff, created_at)| {
                    created_at <= cutoff
                        && (!is_canonical_opencode_db_event
                            || scope.disabled_from.is_none()
                            || scope.disabled_until.is_none())
                });
            let disabled_window_match = match (
                scope.disabled_from.as_deref(),
                scope.disabled_until.as_deref(),
                event.created_at.as_deref(),
            ) {
                (Some(disabled_from), Some(disabled_until), Some(created_at)) => {
                    timestamp_is_in_disabled_window(created_at, disabled_from, disabled_until)
                }
                _ => false,
            };
            cutoff_match || disabled_window_match
        });
        if legacy_unscoped_match || scoped_match {
            return true;
        }
        if provider_source_key.is_some() {
            return false;
        }
        let Some(cutoff) = self.skip_events_at_or_before.as_deref() else {
            return false;
        };
        event
            .created_at
            .as_deref()
            .is_some_and(|created_at| created_at <= cutoff)
    }
}

fn timestamp_is_in_disabled_window(
    created_at: &str,
    disabled_from: &str,
    disabled_until: &str,
) -> bool {
    let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return false;
    };
    let Ok(disabled_from) = chrono::DateTime::parse_from_rfc3339(disabled_from) else {
        return false;
    };
    let Ok(disabled_until) = chrono::DateTime::parse_from_rfc3339(disabled_until) else {
        return false;
    };
    created_at > disabled_from && created_at <= disabled_until
}

pub fn effective_conversation_logging(
    global: ConversationLoggingSetting,
    agent: AgentConversationLoggingSetting,
) -> ConversationLoggingSetting {
    match agent {
        AgentConversationLoggingSetting::Default => global,
        AgentConversationLoggingSetting::Enabled => ConversationLoggingSetting::Enabled,
        AgentConversationLoggingSetting::Disabled => ConversationLoggingSetting::Disabled,
    }
}

impl ConversationArchiveState {
    /// Records the provider-session boundary before startup events are
    /// emitted. This remains available when conversation logging is disabled,
    /// because live Chat still projects memory activity.
    pub fn begin_live_conversation(&self, agent_id: &str, started_at: &str) -> io::Result<()> {
        let agent_id = agent_id.trim();
        let started_at = started_at.trim();
        if agent_id.is_empty() || started_at.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent_id and started_at are required",
            ));
        }
        self.live_started_at
            .lock()
            .map_err(|_| io::Error::other("live conversation boundary lock poisoned"))?
            .insert(agent_id.to_string(), started_at.to_string());
        Ok(())
    }

    pub fn live_conversation_started_at(&self, agent_id: &str) -> io::Result<Option<String>> {
        Ok(self
            .live_started_at
            .lock()
            .map_err(|_| io::Error::other("live conversation boundary lock poisoned"))?
            .get(agent_id.trim())
            .cloned())
    }

    pub fn active_conversation_id(&self, agent_id: &str) -> io::Result<Option<String>> {
        Ok(lock_active(&self.active)?
            .get(agent_id)
            .map(|handle| handle.conversation_id.clone()))
    }

    pub fn list(
        &self,
        agent: Option<&str>,
        scope_all: bool,
    ) -> io::Result<Vec<ConversationIndexEntry>> {
        if let Some(agent_id) = agent.map(str::trim).filter(|agent_id| !agent_id.is_empty()) {
            return read_agent_index(agent_id);
        }

        if scope_all {
            return read_all_agent_indexes();
        }

        let current_agent = std::env::var("WARDIAN_SESSION_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "conversation list requires an agent or scope_all=true when WARDIAN_SESSION_ID is not set",
                )
            })?;
        read_agent_index(&current_agent)
    }

    pub fn show(
        &self,
        conversation_id: &str,
    ) -> io::Result<(ConversationManifest, Vec<ConversationNarrativeRecord>)> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "conversation_id is required",
            ));
        }

        let entry = read_all_agent_indexes()?
            .into_iter()
            .find(|entry| entry.conversation_id == conversation_id)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("conversation not found: {conversation_id}"),
                )
            })?;
        let agent_lock = agent_lock_for(&self.agent_locks, &entry.agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        let conversation_dir = conversation_dir(&entry.agent_id, &entry.conversation_id)?;
        let manifest =
            read_manifest(&conversation_dir.join("manifest.json"))?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("conversation manifest not found: {conversation_id}"),
                )
            })?;
        let conversation = read_jsonl_records(&conversation_dir.join("conversation.jsonl"))?;

        Ok((manifest, conversation))
    }

    /// Reads the already-materialized turn records for the supplied archive
    /// entries. Change review uses this rather than re-deriving turns from
    /// provider transcripts.
    pub fn turn_records_for_conversations(
        &self,
        entries: &[ConversationIndexEntry],
    ) -> io::Result<Vec<(ConversationIndexEntry, ConversationTurnRecord)>> {
        let mut records = Vec::new();
        for entry in entries {
            let agent_lock = agent_lock_for(&self.agent_locks, &entry.agent_id)?;
            let _guard = lock_agent_archive(&agent_lock)?;
            let directory = conversation_dir(&entry.agent_id, &entry.conversation_id)?;
            let turns: Vec<ConversationTurnRecord> =
                read_jsonl_records(&directory.join("turns.jsonl"))?;
            records.extend(turns.into_iter().map(|turn| (entry.clone(), turn)));
        }
        Ok(records)
    }

    /// Reads materialized turn records for change review. Unlike the shared
    /// archive readers, malformed turn records are skipped and counted so one
    /// legacy line cannot blank the Git-derived change set.
    pub fn turn_records_for_conversations_resilient(
        &self,
        entries: &[ConversationIndexEntry],
    ) -> io::Result<(Vec<(ConversationIndexEntry, ConversationTurnRecord)>, usize)> {
        let mut records = Vec::new();
        let mut skipped_records = 0;
        for entry in entries {
            let agent_lock = agent_lock_for(&self.agent_locks, &entry.agent_id)?;
            let _guard = lock_agent_archive(&agent_lock)?;
            let directory = conversation_dir(&entry.agent_id, &entry.conversation_id)?;
            let (turns, skipped) = read_jsonl_records_resilient(&directory.join("turns.jsonl"))?;
            skipped_records += skipped;
            records.extend(turns.into_iter().map(|turn| (entry.clone(), turn)));
        }
        Ok((records, skipped_records))
    }

    /// Returns the persisted chat events for every archived conversation owned
    /// by one agent, oldest conversation first. The live chat surface uses
    /// this as durable history when a provider log rotates or is unavailable.
    pub fn chat_events_for_agent(&self, agent_id: &str) -> io::Result<Vec<AgentChatEvent>> {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent_id is required",
            ));
        }

        let mut entries = read_agent_index(agent_id)?;
        entries.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then_with(|| left.conversation_id.cmp(&right.conversation_id))
        });

        let mut events = Vec::new();
        for entry in entries {
            let directory = conversation_dir(&entry.agent_id, &entry.conversation_id)?;
            let mut conversation_events: Vec<AgentChatEvent> = read_chat_events(&directory)?;
            for event in &mut conversation_events {
                if let Some(metadata) = event.metadata.as_object_mut() {
                    metadata.insert(
                        "conversation_archive_id".to_string(),
                        serde_json::Value::String(entry.conversation_id.clone()),
                    );
                }
            }
            events.extend(conversation_events);
        }

        Ok(events)
    }

    /// Returns persisted chat events for the agent's open conversation only.
    /// The live chat surface must not replay a closed conversation after a
    /// user starts a new provider session.
    pub fn chat_events_for_active_conversation(
        &self,
        agent_id: &str,
    ) -> io::Result<Vec<AgentChatEvent>> {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent_id is required",
            ));
        }

        let Some(handle) = lock_active(&self.active)?.get(agent_id).cloned() else {
            return Ok(Vec::new());
        };
        let directory = conversation_dir(agent_id, &handle.conversation_id)?;
        let mut events: Vec<AgentChatEvent> = read_chat_events(&directory)?;
        for event in &mut events {
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.insert(
                    "conversation_archive_id".to_string(),
                    serde_json::Value::String(handle.conversation_id.clone()),
                );
            }
        }
        Ok(events)
    }

    /// Read the open archive for an explicitly bound capture, including after
    /// logging was disabled and its in-memory active handle was discarded.
    /// Unknown sources and closed conversations are never resurrected.
    pub fn chat_events_for_capture(
        &self,
        context: &ConversationArchiveContext,
    ) -> io::Result<Vec<AgentChatEvent>> {
        if context.provider_source_key.is_none() {
            return self.chat_events_for_active_conversation(&context.agent_id);
        }
        let Some(source) = context.provider_source_key.as_deref() else {
            return Ok(Vec::new());
        };
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        for entry in read_agent_index(&context.agent_id)? {
            let directory = conversation_dir(&context.agent_id, &entry.conversation_id)?;
            let Some(manifest) = read_manifest(&directory.join("manifest.json"))? else {
                continue;
            };
            if manifest.status != wardian_core::conversations::ConversationStatus::Open
                || manifest.provider != context.provider
                || manifest.provider_source_key.as_deref() != Some(source)
            {
                continue;
            }
            let mut events: Vec<AgentChatEvent> = read_chat_events(&directory)?;
            for event in &mut events {
                event.metadata["conversation_archive_id"] =
                    serde_json::json!(entry.conversation_id);
            }
            return Ok(events);
        }
        Ok(Vec::new())
    }

    pub fn append_chat_events(
        &self,
        agent_id: &str,
        events: &[AgentChatEvent],
    ) -> io::Result<usize> {
        let provider = provider_from_events(events).unwrap_or_else(|| "unknown".to_string());
        self.append_chat_events_with_context(
            ConversationArchiveContext::for_agent_id(agent_id, &provider),
            events,
        )
    }

    pub fn append_chat_events_with_context(
        &self,
        context: ConversationArchiveContext,
        events: &[AgentChatEvent],
    ) -> io::Result<usize> {
        if !events
            .iter()
            .any(|event| record_kind_from_chat_event_kind(&event.kind).is_some())
        {
            return Ok(0);
        }

        if events
            .iter()
            .any(|event| event.session_id != context.agent_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "capture events must belong to the archive agent",
            ));
        }
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        self.append_chat_events_with_context_locked(context, events)
    }

    fn append_chat_events_with_context_locked(
        &self,
        mut context: ConversationArchiveContext,
        events: &[AgentChatEvent],
    ) -> io::Result<usize> {
        if !events
            .iter()
            .any(|event| record_kind_from_chat_event_kind(&event.kind).is_some())
        {
            return Ok(0);
        }
        let provider_source_key = context
            .provider_source_key
            .clone()
            .or_else(|| provider_source_key_from_events(events));
        if context.provider_source_key.is_none() {
            context.provider_source_key = provider_source_key.clone();
        }
        if context.provider_session_ids.is_empty() {
            context.provider_session_ids = provider_session_ids_from_events(events);
        }
        let capture_state = read_capture_state(&context.agent_id)?;
        let active_events = events
            .iter()
            .filter(|event| !capture_state.should_skip_event(event, provider_source_key.as_deref()))
            .cloned()
            .collect::<Vec<_>>();
        let batch_events = coalesce_batch_observations(&active_events)?;
        let mut handle =
            active_handle_for_context(&self.active, &context, provider_source_key.clone())?;
        let conversation_dir = conversation_dir(&context.agent_id, &handle.conversation_id)?;
        let effective_context = effective_context_for_handle(&context, &handle, &conversation_dir)?;
        let conversation_path = conversation_dir.join("conversation.jsonl");
        let events_path = conversation_dir.join("events.jsonl");
        let sources_path = conversation_dir.join("sources.jsonl");
        let mut existing_records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path)?;
        let mut existing_events: Vec<AgentChatEvent> = read_jsonl_records(&events_path)?;
        let before_refresh_events = existing_events.clone();
        let before_refresh_records = existing_records.clone();
        let durable_event_ids = existing_events
            .iter()
            .flat_map(|event| event_identity_ids(event))
            .map(ToString::to_string)
            .collect::<HashSet<_>>();
        // Cutoffs suppress new capture, not enrichment of an observation
        // already archived while logging was enabled. This never adds a row.
        let events_refreshed = provenance::refresh_events(&mut existing_events, events)?;
        let delivered_refreshed =
            provenance::bind_delivered_inputs(&mut existing_events, &existing_records)?;
        let events_refreshed = events_refreshed || delivered_refreshed;
        let observed = existing_events
            .iter()
            .filter(|event| {
                events
                    .iter()
                    .any(|current| provenance::same_observation(event, current))
            })
            .cloned()
            .collect::<Vec<_>>();
        provenance::refresh_records(&mut existing_records, &observed);
        for record in &mut existing_records {
            if !before_refresh_records.contains(record) {
                materialize_record_text(&conversation_dir, record)?;
            }
        }
        for event in &mut existing_events {
            if provenance::completed_native_tool(event) {
                if let Some(record) = existing_records
                    .iter()
                    .find(|record| record.event_refs.contains(&event.id))
                {
                    *event = event_record_for_jsonl(event, record);
                }
            }
        }
        let records_refreshed = before_refresh_records != existing_records;
        let refreshed = events_refreshed || records_refreshed;
        let mut changed_observation_ids = provenance::changed_observation_ids(
            &before_refresh_events,
            &before_refresh_records,
            &existing_events,
            &existing_records,
            events,
        );
        // Each file keeps its previous snapshot on failed publication. Retry
        // also repairs a narrative left behind after events were published.
        let mut next_seq = handle.next_seq.max(
            existing_records
                .iter()
                .map(|record| record.seq)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        let recovered_observations = recover_unlinked_observations(
            &handle.conversation_id,
            &conversation_dir,
            &effective_context,
            &existing_records,
            &existing_events,
            Some(&durable_event_ids),
            &mut next_seq,
        )?;
        if events_refreshed {
            write_jsonl_atomic(&events_path, &existing_events)?;
        }
        let recovered_count = publish_recovered_observations(
            &conversation_path,
            &sources_path,
            &mut existing_records,
            recovered_observations,
        )?;
        if records_refreshed && recovered_count == 0 {
            write_jsonl_atomic(&conversation_path, &existing_records)?;
        }
        let mut appended = Vec::new();
        let mut pending_publications = Vec::new();
        let mut cached_sources = None;
        let mut merged_existing_count = 0_usize;

        for event in &batch_events {
            let existing_event_index = matching_event_index(&existing_events, event)?;
            let matching_record = matching_record_index(&existing_records, event)?;
            if existing_event_index.is_none()
                && matching_record
                    .is_some_and(|index| !is_bound_native_delivery(&existing_records[index], event))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "event identity is referenced without its durable observation: {}",
                        event.id
                    ),
                ));
            }
            if let Some(record_index) = matching_record {
                let record_seq = existing_records[record_index].seq;
                let source_record = source_record_from_chat_event(event, record_seq);
                let mut record_changed = false;
                if let Some(source_record) = source_record {
                    let source_repaired = append_source_if_needed(
                        &sources_path,
                        &mut cached_sources,
                        &source_record,
                        true,
                    )?;
                    record_changed |= source_repaired;
                    if !existing_records[record_index]
                        .source_refs
                        .iter()
                        .any(|source_ref| source_ref == &source_record.source_id)
                    {
                        existing_records[record_index]
                            .source_refs
                            .push(source_record.source_id.clone());
                        record_changed = true;
                    }
                }
                let durable_event_id = existing_event_index
                    .and_then(|index| existing_events.get(index))
                    .map(|event| event.id.clone())
                    .unwrap_or_else(|| event.id.clone());
                if !existing_records[record_index]
                    .event_refs
                    .iter()
                    .any(|event_ref| event_ref == &durable_event_id)
                {
                    existing_records[record_index]
                        .event_refs
                        .push(durable_event_id.clone());
                    record_changed = true;
                }
                if existing_records[record_index].turn_id.is_none() && event.turn_id.is_some() {
                    existing_records[record_index].turn_id = event.turn_id.clone();
                    record_changed = true;
                }
                if existing_records[record_index].speaker_type
                    == Some(ConversationSpeakerType::Unknown)
                {
                    existing_records[record_index].speaker_type =
                        Some(ConversationSpeakerType::User);
                    record_changed = true;
                }
                let bound_native_delivery =
                    is_bound_native_delivery(&existing_records[record_index], event);
                if existing_event_index.is_none() && !bound_native_delivery {
                    let event_record =
                        event_record_for_jsonl(event, &existing_records[record_index]);
                    append_jsonl_record(&events_path, &event_record)?;
                    record_changed = true;
                }
                if record_changed {
                    merged_existing_count = merged_existing_count.saturating_add(1);
                    changed_observation_ids.insert(event.id.clone());
                }
                continue;
            }
            if let Some(record_index) =
                matching_delivered_input_record_index(&existing_records, event)?
            {
                let record_seq = existing_records[record_index].seq;
                let source_record = source_record_from_chat_event(event, record_seq);
                if let Some(source_record) = source_record {
                    append_source_if_needed(
                        &sources_path,
                        &mut cached_sources,
                        &source_record,
                        true,
                    )?;
                    if !existing_records[record_index]
                        .source_refs
                        .iter()
                        .any(|source_ref| source_ref == &source_record.source_id)
                    {
                        existing_records[record_index]
                            .source_refs
                            .push(source_record.source_id.clone());
                    }
                }
                existing_records[record_index]
                    .event_refs
                    .push(event.id.clone());
                if existing_records[record_index].turn_id.is_none() {
                    existing_records[record_index].turn_id = event.turn_id.clone();
                }
                if existing_records[record_index].speaker_type
                    == Some(ConversationSpeakerType::Unknown)
                {
                    existing_records[record_index].speaker_type =
                        Some(ConversationSpeakerType::User);
                }
                if existing_event_index.is_none() {
                    let event_record =
                        event_record_for_jsonl(event, &existing_records[record_index]);
                    append_jsonl_record(&events_path, &event_record)?;
                }
                merged_existing_count = merged_existing_count.saturating_add(1);
                changed_observation_ids.insert(event.id.clone());
                continue;
            }
            let Some(mut record) = narrative_from_chat_event(event, next_seq) else {
                continue;
            };
            materialize_record_text(&conversation_dir, &mut record)?;
            let durable_event_id = existing_event_index
                .and_then(|index| existing_events.get(index))
                .map(|event| event.id.clone())
                .unwrap_or_else(|| event.id.clone());
            record.event_refs = vec![durable_event_id];
            let source_record = source_record_from_chat_event(event, next_seq);
            if let Some(source_record) = &source_record {
                record.source_refs = vec![source_record.source_id.clone()];
            }
            pending_publications.push(PendingChatPublication {
                event: existing_event_index
                    .is_none()
                    .then(|| event_record_for_jsonl(event, &record)),
                source: source_record,
                record: record.clone(),
            });
            next_seq = next_seq.saturating_add(1);
            appended.push(record);
        }

        if appended.is_empty()
            && merged_existing_count == 0
            && recovered_count == 0
            && !refreshed
            && (observed.is_empty()
                || projection_is_current(
                    &conversation_dir,
                    &effective_context,
                    &handle,
                    &existing_records,
                    &existing_events,
                )?)
        {
            handle.next_seq = next_seq;
            lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
            return Ok(0);
        }

        if merged_existing_count > 0 {
            write_jsonl_atomic(&conversation_path, &existing_records)?;
        }

        for publication in &pending_publications {
            if let Some(event_record) = &publication.event {
                append_jsonl_record(&events_path, event_record)?;
            }
            if let Some(source_record) = &publication.source {
                append_source_if_needed(
                    &sources_path,
                    &mut cached_sources,
                    source_record,
                    publication.event.is_none(),
                )?;
            }
            append_jsonl_record(&conversation_path, &publication.record)?;
        }

        let first_record = existing_records
            .first()
            .or_else(|| appended.first())
            .expect("appended records are non-empty");
        let last_record = appended
            .last()
            .or_else(|| existing_records.last())
            .expect("conversation has at least one record");
        let all_records = existing_records
            .iter()
            .chain(appended.iter())
            .cloned()
            .collect::<Vec<_>>();
        let mut all_events: Vec<AgentChatEvent> = read_jsonl_records(&events_path)?;
        if provenance::bind_delivered_inputs(&mut all_events, &all_records)? {
            write_jsonl_atomic(&events_path, &all_events)?;
        }
        let all_sources: Vec<ConversationSourceRecord> = read_jsonl_records(&sources_path)?;
        let turns = derive_turn_records_with_context(
            &handle.conversation_id,
            &all_records,
            &all_events,
            &all_sources,
            true,
            Some(&effective_context.provider),
            &effective_context.provider_session_ids,
        );
        write_jsonl_atomic(&conversation_dir.join("turns.jsonl"), &turns)?;
        let summary = archive_summary(&all_records, &turns, &all_sources);
        let record_count = all_records.len() as u64;
        let mut manifest = open_manifest(
            &effective_context,
            &handle.conversation_id,
            first_record.at.clone(),
            last_record.at.clone(),
        );
        apply_archive_summary_to_manifest(&mut manifest, &summary);
        write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
        append_index_upsert(
            &index_path(&context.agent_id)?,
            &index_entry_from_manifest(
                &manifest,
                None,
                excerpt_from_record(first_record),
                excerpt_from_record(last_record),
                record_count,
                artifact_count_for_records(all_records.iter()),
            ),
        )?;

        handle.next_seq = next_seq;
        lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
        Ok(appended
            .len()
            .saturating_add(changed_observation_ids.len())
            .saturating_add(recovered_count))
    }

    pub(crate) fn provider_log_capture_state(
        &self,
        agent_id: &str,
        provider_source_key: &str,
    ) -> io::Result<Option<crate::commands::provider_log_acquisition::ProviderLogCaptureState>>
    {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        Ok(read_capture_state(agent_id)?
            .provider_log_sources
            .into_iter()
            .find(|state| state.provider_source_key == provider_source_key))
    }

    /// Publishes one provider-log batch and advances its private cursor as one
    /// per-agent operation. The archive append must succeed before the cursor
    /// compare-and-set is written. Multi-file archive publication remains
    /// retryable rather than transactional; that separate limit is #1183.
    pub(crate) fn append_provider_log_batch_with_context(
        &self,
        context: ConversationArchiveContext,
        events: &[AgentChatEvent],
        expected: Option<&crate::commands::provider_log_acquisition::ProviderLogCaptureState>,
        next: &crate::commands::provider_log_acquisition::ProviderLogCaptureState,
    ) -> io::Result<usize> {
        if events
            .iter()
            .any(|event| event.session_id != context.agent_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "capture events must belong to the archive agent",
            ));
        }
        if context.provider_source_key.as_deref() != Some(next.provider_source_key.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "provider-log cursor source does not match archive context",
            ));
        }

        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let mut capture_state = read_capture_state(&context.agent_id)?;
        let current_index = capture_state
            .provider_log_sources
            .iter()
            .position(|state| state.provider_source_key == next.provider_source_key);
        let current = current_index.map(|index| &capture_state.provider_log_sources[index]);
        if current != expected {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "provider-log capture state changed before cursor commit",
            ));
        }
        let appended = self.append_chat_events_with_context_locked(context.clone(), events)?;
        if let Some(index) = current_index {
            capture_state.provider_log_sources[index] = next.clone();
        } else {
            capture_state.provider_log_sources.push(next.clone());
        }
        write_capture_state(&context.agent_id, &capture_state)?;
        Ok(appended)
    }

    pub fn append_delivered_input(
        &self,
        agent_id: &str,
        text: &str,
        sender_agent_id: Option<&str>,
    ) -> io::Result<usize> {
        self.append_delivered_input_with_context(
            ConversationArchiveContext::for_agent_id(agent_id, "unknown"),
            text,
            sender_agent_id,
        )
    }

    pub fn append_delivered_input_with_context(
        &self,
        context: ConversationArchiveContext,
        text: &str,
        sender_agent_id: Option<&str>,
    ) -> io::Result<usize> {
        if text.trim().is_empty() {
            return Ok(0);
        }

        self.append_generated_record(context, |seq| {
            narrative_from_delivered_input(&current_rfc3339_millis(), text, sender_agent_id, seq)
        })
    }

    pub fn append_lifecycle_boundary(
        &self,
        agent_id: &str,
        reason: ConversationBoundaryReason,
    ) -> io::Result<usize> {
        self.append_lifecycle_boundary_with_context(
            ConversationArchiveContext::for_agent_id(agent_id, "unknown"),
            reason,
        )
    }

    pub fn append_lifecycle_boundary_with_context(
        &self,
        context: ConversationArchiveContext,
        reason: ConversationBoundaryReason,
    ) -> io::Result<usize> {
        self.append_generated_record(context, |seq| {
            lifecycle_record(seq, reason, &current_rfc3339_millis())
        })
    }

    pub fn active_ends_with_lifecycle_boundary(
        &self,
        agent_id: &str,
        reason: ConversationBoundaryReason,
    ) -> io::Result<bool> {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let Some(handle) = lock_active(&self.active)?.get(agent_id).cloned() else {
            return Ok(false);
        };
        let conversation_path =
            conversation_dir(agent_id, &handle.conversation_id)?.join("conversation.jsonl");
        let records: Vec<ConversationNarrativeRecord> = read_jsonl_records(&conversation_path)?;
        let expected_status = serde_json::to_value(reason)
            .map_err(io::Error::other)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| io::Error::other("conversation boundary reason was not a string"))?;
        Ok(records.last().is_some_and(|record| {
            record.kind == ConversationRecordKind::Lifecycle
                && record.status.as_deref() == Some(expected_status.as_str())
        }))
    }

    fn append_generated_record(
        &self,
        mut context: ConversationArchiveContext,
        make_record: impl FnOnce(u64) -> ConversationNarrativeRecord,
    ) -> io::Result<usize> {
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let provider_source_key = context.provider_source_key.clone();
        if context.provider_source_key.is_none() {
            context.provider_source_key = provider_source_key.clone();
        }
        let mut handle = active_handle_for_context(&self.active, &context, provider_source_key)?;
        let conversation_dir = conversation_dir(&context.agent_id, &handle.conversation_id)?;
        let effective_context = effective_context_for_handle(&context, &handle, &conversation_dir)?;
        let conversation_path = conversation_dir.join("conversation.jsonl");
        let events_path = conversation_dir.join("events.jsonl");
        let sources_path = conversation_dir.join("sources.jsonl");
        let mut existing_records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path)?;
        let existing_events: Vec<AgentChatEvent> = read_jsonl_records(&events_path)?;
        let mut next_seq = handle.next_seq.max(
            existing_records
                .iter()
                .map(|record| record.seq)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        let recovered_observations = recover_unlinked_observations(
            &handle.conversation_id,
            &conversation_dir,
            &effective_context,
            &existing_records,
            &existing_events,
            None,
            &mut next_seq,
        )?;
        publish_recovered_observations(
            &conversation_path,
            &sources_path,
            &mut existing_records,
            recovered_observations,
        )?;
        if !existing_records.is_empty()
            && !projection_is_current(
                &conversation_dir,
                &effective_context,
                &handle,
                &existing_records,
                &existing_events,
            )?
        {
            rebuild_derived_projections(
                &context.agent_id,
                &conversation_dir,
                &effective_context,
                &handle,
                &existing_records,
                &existing_events,
            )?;
        }

        let mut candidate = make_record(next_seq);
        materialize_record_text(&conversation_dir, &mut candidate)?;
        let candidate_sources = generated_sources_from_record(&effective_context, &mut candidate);
        let candidate_event = generated_event_from_record(
            &effective_context,
            &handle.conversation_id,
            &mut candidate,
        );
        let existing_record_index = existing_records
            .iter()
            .position(|record| record.event_refs.contains(&candidate_event.id));
        let record_is_new = existing_record_index.is_none();
        let mut record = existing_record_index
            .and_then(|index| existing_records.get(index).cloned())
            .unwrap_or(candidate);
        let generated_sources = if record_is_new {
            candidate_sources
        } else {
            generated_sources_from_record(&effective_context, &mut record)
        };
        let generated_event = candidate_event;
        let mut cached_sources = None;
        if record_is_new {
            append_jsonl_record(&events_path, &generated_event)?;
        }
        for source in &generated_sources {
            append_source_if_needed(&sources_path, &mut cached_sources, source, !record_is_new)?;
        }
        if record_is_new {
            append_jsonl_record(&conversation_path, &record)?;
        }

        let all_records = if record_is_new {
            existing_records
                .iter()
                .chain(std::iter::once(&record))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            existing_records.clone()
        };
        let mut all_events: Vec<AgentChatEvent> = read_jsonl_records(&events_path)?;
        if provenance::bind_delivered_inputs(&mut all_events, &all_records)? {
            write_jsonl_atomic(&events_path, &all_events)?;
        }
        rebuild_derived_projections(
            &context.agent_id,
            &conversation_dir,
            &effective_context,
            &handle,
            &all_records,
            &all_events,
        )?;

        handle.next_seq = if record_is_new {
            next_seq.saturating_add(1)
        } else {
            handle.next_seq.max(next_seq)
        };
        lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
        Ok(usize::from(record_is_new))
    }

    pub fn rollover_agent(
        &self,
        agent_id: &str,
        reason: ConversationBoundaryReason,
    ) -> io::Result<Option<String>> {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let Some(handle) = lock_active(&self.active)?.get(agent_id).cloned() else {
            return Ok(None);
        };
        let conversation_dir = conversation_dir(agent_id, &handle.conversation_id)?;
        close_conversation_dir(agent_id, &handle.conversation_id, &conversation_dir, reason)?;
        #[cfg(test)]
        if self
            .fail_next_rollover_after_close
            .swap(false, Ordering::SeqCst)
        {
            return Err(io::Error::other("injected rollover failure after close"));
        }
        let mut active = lock_active(&self.active)?;
        if active
            .get(agent_id)
            .is_some_and(|current| current.conversation_id == handle.conversation_id)
        {
            active.remove(agent_id);
        }
        Ok(Some(handle.conversation_id))
    }

    pub fn discard_agent(&self, agent_id: &str) -> io::Result<Option<String>> {
        self.discard_agent_with_events(agent_id, &[])
    }

    pub fn discard_agent_with_events(
        &self,
        agent_id: &str,
        events: &[AgentChatEvent],
    ) -> io::Result<Option<String>> {
        self.discard_agent_capture(agent_id, None, events)
    }

    pub fn discard_agent_with_context(
        &self,
        context: ConversationArchiveContext,
        events: &[AgentChatEvent],
    ) -> io::Result<Option<String>> {
        self.discard_agent_capture(
            &context.agent_id,
            context.provider_source_key.as_deref(),
            events,
        )
    }

    fn discard_agent_capture(
        &self,
        agent_id: &str,
        provider_source_key: Option<&str>,
        events: &[AgentChatEvent],
    ) -> io::Result<Option<String>> {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let removed = lock_active(&self.active)?
            .get(agent_id)
            .map(|handle| handle.conversation_id.clone());
        let mut capture_state = read_capture_state(agent_id)?;
        let cutoff = current_rfc3339_millis();

        if let Some(provider_source_key) = provider_source_key {
            let provider_source_key = Some(provider_source_key.to_string());
            let scope_index = capture_state
                .skip_event_scopes
                .iter()
                .position(|scope| scope.provider_source_key == provider_source_key)
                .unwrap_or_else(|| {
                    capture_state
                        .skip_event_scopes
                        .push(ConversationCaptureEventScope {
                            provider_source_key: provider_source_key.clone(),
                            skip_events_at_or_before: None,
                            disabled_from: None,
                            disabled_until: None,
                            event_ids: Vec::new(),
                        });
                    capture_state.skip_event_scopes.len() - 1
                });
            let scope = &mut capture_state.skip_event_scopes[scope_index];
            scope.skip_events_at_or_before = Some(cutoff);
            if scope.disabled_from.is_none() || scope.disabled_until.is_some() {
                scope.disabled_from = scope.skip_events_at_or_before.clone();
                scope.disabled_until = None;
            }
            let mut seen = scope.event_ids.iter().cloned().collect::<HashSet<_>>();
            for event in events {
                for event_id in event_identity_ids(event) {
                    if !event_id.trim().is_empty() && seen.insert(event_id.to_string()) {
                        scope.event_ids.push(event_id.to_string());
                    }
                }
            }
        } else {
            capture_state.skip_events_at_or_before = Some(cutoff);
            let mut seen = capture_state
                .skip_event_ids
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            for event in events {
                for event_id in event_identity_ids(event) {
                    if !event_id.trim().is_empty() && seen.insert(event_id.to_string()) {
                        capture_state.skip_event_ids.push(event_id.to_string());
                    }
                }
            }
        }
        write_capture_state(agent_id, &capture_state)?;
        let mut active = lock_active(&self.active)?;
        if active
            .get(agent_id)
            .is_some_and(|handle| Some(handle.conversation_id.as_str()) == removed.as_deref())
        {
            active.remove(agent_id);
        }
        Ok(removed)
    }

    pub(crate) fn close_agent_capture_disabled_window(
        &self,
        context: ConversationArchiveContext,
    ) -> io::Result<()> {
        let Some(provider_source_key) = context.provider_source_key else {
            return Ok(());
        };
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let mut capture_state = read_capture_state(&context.agent_id)?;
        let Some(scope) = capture_state.skip_event_scopes.iter_mut().find(|scope| {
            scope.provider_source_key.as_deref() == Some(provider_source_key.as_str())
        }) else {
            return Ok(());
        };
        if scope.disabled_from.is_some() && scope.disabled_until.is_none() {
            scope.disabled_until = Some(current_rfc3339_millis());
            write_capture_state(&context.agent_id, &capture_state)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn set_active_for_test(&self, agent_id: &str, handle: ActiveConversationHandle) {
        self.active
            .lock()
            .expect("active conversation lock")
            .insert(agent_id.to_string(), handle);
    }

    #[cfg(test)]
    pub fn fail_next_rollover_after_close_for_test(&self) {
        self.fail_next_rollover_after_close
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn active_conversation_id_for_test(&self, agent_id: &str) -> Option<String> {
        self.active_conversation_id(agent_id)
            .expect("active conversation lock")
    }
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
#[cfg(test)]
mod completion_tests;
#[cfg(test)]
mod provenance_tests;
#[cfg(test)]
mod repair_tests;

fn read_chat_events(directory: &std::path::Path) -> io::Result<Vec<AgentChatEvent>> {
    let mut events = read_jsonl_records(&directory.join("events.jsonl"))?;
    let records = read_jsonl_records(&directory.join("conversation.jsonl"))?;
    provenance::bind_delivered_inputs(&mut events, &records)?;
    Ok(events)
}

// A previous repair may have published events/narrative before a derived file
// failed. Do not let the ordinary duplicate fast path strand that snapshot.
fn projection_is_current(
    directory: &std::path::Path,
    context: &ConversationArchiveContext,
    handle: &ActiveConversationHandle,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
) -> io::Result<bool> {
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        return Ok(true);
    };
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&directory.join("sources.jsonl"))?;
    let turns = derive_turn_records_with_context(
        &handle.conversation_id,
        records,
        events,
        &sources,
        true,
        Some(&context.provider),
        &context.provider_session_ids,
    );
    let stored_turns: Vec<ConversationTurnRecord> =
        read_jsonl_records(&directory.join("turns.jsonl"))?;
    if turns != stored_turns {
        return Ok(false);
    }
    let mut manifest = open_manifest(
        context,
        &handle.conversation_id,
        first.at.clone(),
        last.at.clone(),
    );
    apply_archive_summary_to_manifest(&mut manifest, &archive_summary(records, &turns, &sources));
    if read_manifest(&directory.join("manifest.json"))?.as_ref() != Some(&manifest) {
        return Ok(false);
    }
    let expected = index_entry_from_manifest(
        &manifest,
        None,
        excerpt_from_record(first),
        excerpt_from_record(last),
        records.len() as u64,
        artifact_count_for_records(records.iter()),
    );
    Ok(read_agent_index(&context.agent_id)?
        .iter()
        .any(|entry| entry == &expected))
}
