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

mod chat_logical_index;
pub(crate) mod chat_read;
mod chat_read_store;
mod chat_source_index;
mod compatibility_claims;
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
    pub(crate) chat_projection: chat_read::ProjectionOwner,
    deferred_receipt_summaries: Mutex<HashMap<String, String>>,
    #[cfg(test)]
    fail_next_rollover_after_close: AtomicBool,
    #[cfg(test)]
    fail_next_chat_cursor_commit: AtomicBool,
    #[cfg(test)]
    fail_compatibility_stage: std::sync::atomic::AtomicU8,
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
    compatibility_claims: Option<compatibility_claims::Checkpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_events_at_or_before: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skip_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skip_event_scopes: Vec<ConversationCaptureEventScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    provider_log_sources: Vec<crate::commands::provider_log_acquisition::ProviderLogCaptureState>,
}

struct CapturePreparation<'a> {
    state: &'a mut ConversationCaptureState,
    previous: Option<&'a crate::commands::provider_log_acquisition::ProviderLogCaptureState>,
    next: &'a crate::commands::provider_log_acquisition::ProviderLogCaptureState,
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
        let (count, candidate) = self.append_chat_events_with_context_locked(context, events)?;
        if let Some(candidate) = candidate {
            self.publish_chat_candidate(candidate)?;
        }
        Ok(count)
    }

    fn append_chat_events_with_context_locked(
        &self,
        context: ConversationArchiveContext,
        events: &[AgentChatEvent],
    ) -> io::Result<(usize, Option<chat_read::Candidate>)> {
        self.append_chat_events_prepared_locked(context, events, None)
    }

    fn append_chat_events_prepared_locked(
        &self,
        mut context: ConversationArchiveContext,
        events: &[AgentChatEvent],
        capture: Option<CapturePreparation<'_>>,
    ) -> io::Result<(usize, Option<chat_read::Candidate>)> {
        let recovering = if let Some(capture) = capture.as_ref() {
            compatibility_claims::pending(capture.state, &context.agent_id)?
        } else {
            false
        };
        if !events
            .iter()
            .any(|event| record_kind_from_chat_event_kind(&event.kind).is_some())
            && !recovering
        {
            return Ok((0, None));
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
        let (batch_events, compatibility_overlay) = if let Some(capture) = capture {
            #[cfg(test)]
            if self
                .fail_compatibility_stage
                .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Err(io::Error::other(
                    "injected compatibility failure before prepare",
                ));
            }
            let manifest = read_manifest(&conversation_dir.join("manifest.json"))?;
            let prepared = compatibility_claims::prepare(
                compatibility_claims::Preparation {
                    context: &effective_context,
                    conversation_id: &handle.conversation_id,
                    manifest: manifest.as_ref(),
                    archived: &existing_events,
                    records: &existing_records,
                    events: &batch_events,
                    previous: capture.previous,
                    next: capture.next,
                },
                capture.state,
            )?;
            #[cfg(test)]
            if self
                .fail_compatibility_stage
                .compare_exchange(2, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Err(io::Error::other(
                    "injected compatibility failure after prepare",
                ));
            }
            prepared
        } else {
            (batch_events, HashMap::new())
        };
        let claimed_coordinates = compatibility_overlay
            .values()
            .filter_map(|event| event.metadata["chat_source_ref"].as_str())
            .collect::<HashSet<_>>();
        let enrichment_events = events
            .iter()
            .filter(|event| {
                !event.metadata["chat_source_ref"]
                    .as_str()
                    .is_some_and(|coordinate| claimed_coordinates.contains(coordinate))
            })
            .cloned()
            .collect::<Vec<_>>();
        let events = enrichment_events.as_slice();
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
        let before_refresh_by_seq = index_records_by_sequence(&before_refresh_records);
        for record in &mut existing_records {
            if !record_was_present(record, &before_refresh_by_seq) {
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
            let matching_record =
                matching_record_index(&existing_records, event, &existing_events)?;
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
            let committed_output_bytes = match std::fs::metadata(&events_path) {
                Ok(metadata) => metadata.len(),
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && handle.next_seq <= 1
                        && existing_events.is_empty()
                        && existing_records.is_empty()
                        && read_manifest(&conversation_dir.join("manifest.json"))?
                            .is_none_or(|manifest| manifest.record_count == 0)
                        && chat_read::committed_output_extent(
                            &context.agent_id,
                            &handle.conversation_id,
                        )?
                        .is_none_or(|extent| extent == 0) =>
                {
                    0
                }
                Err(error) => return Err(error),
            };
            let candidate = chat_read::Candidate {
                context: effective_context,
                conversation_id: handle.conversation_id.clone(),
                events: compatibility_claims::overlay(existing_events, &compatibility_overlay),
                committed_output_bytes,
                verified_ids: events.iter().map(|event| event.id.clone()).collect(),
                generated_input_bindings: chat_read::generated_input_bindings(
                    &existing_records,
                    &handle.conversation_id,
                ),
                generated_ids: existing_records
                    .iter()
                    .flat_map(|record| record.event_refs.iter())
                    .cloned()
                    .collect(),
                source_epoch: None,
            };
            lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
            return Ok((0, Some(candidate)));
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
        let candidate = chat_read::Candidate {
            context: effective_context,
            conversation_id: handle.conversation_id.clone(),
            events: compatibility_claims::overlay(all_events, &compatibility_overlay),
            committed_output_bytes: std::fs::metadata(&events_path)?.len(),
            verified_ids: events.iter().map(|event| event.id.clone()).collect(),
            generated_input_bindings: chat_read::generated_input_bindings(
                &all_records,
                &handle.conversation_id,
            ),
            generated_ids: all_records
                .iter()
                .flat_map(|record| record.event_refs.iter())
                .cloned()
                .collect(),
            source_epoch: None,
        };
        lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
        Ok((
            appended
                .len()
                .saturating_add(changed_observation_ids.len())
                .saturating_add(recovered_count),
            Some(candidate),
        ))
    }

    /// Derived history admission shares archive serialization, but never binds
    /// the mutable writer handle or changes canonical archive files.
    pub(crate) fn bootstrap_saved_chat(
        &self,
        context: &ConversationArchiveContext,
    ) -> io::Result<bool> {
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        self.chat_projection.bootstrap_saved(context)
    }

    fn publish_chat_candidate(&self, mut candidate: chat_read::Candidate) -> io::Result<()> {
        let capture = read_capture_state(&candidate.context.agent_id)?;
        compatibility_claims::restore_candidate(&mut candidate, &capture)?;
        candidate.source_epoch = capture
            .provider_log_sources
            .iter()
            .find(|source| {
                candidate.context.provider_source_key.as_deref()
                    == Some(source.provider_source_key.as_str())
            })
            .map(|source| {
                crate::commands::chat_recent_seed::hash(
                    &serde_json::to_vec(&source.native_identity).unwrap_or_default(),
                )
            });
        // Legacy rows and partially appended batches are not upgraded by a
        // subsequent generated write. Native rows need their committed source
        // coordinate; generated rows need this owned conversation identity.
        let generated_prefix = format!("generated:{}:", candidate.conversation_id);
        for event in &mut candidate.events {
            if event.metadata["generated"] == true {
                // Legacy text-selected links are evidence in the archive, but
                // cannot become verified aliases in the display projection.
                if let Some(metadata) = event.metadata.as_object_mut() {
                    for key in [
                        "chat_source_ref",
                        "chat_source_start",
                        "chat_source_end",
                        "chat_source_epoch",
                        "legacy_event_ids",
                        "request_root_id",
                    ] {
                        metadata.remove(key);
                    }
                    metadata.insert(
                        "chat_identity_resolution".into(),
                        serde_json::json!("unresolved"),
                    );
                }
            }
        }
        let archived_events = std::mem::take(&mut candidate.events);
        let display_events = archived_events
            .into_iter()
            .filter(|event| {
                event.session_id == candidate.context.agent_id
                    && (event.provider == candidate.context.provider
                        || chat_read::is_owned_unknown_input(&candidate, event))
            })
            .collect();
        candidate.events = display_events;
        candidate.events.retain(|event| {
            if event.metadata["generated"] == true {
                return event.id.starts_with(&generated_prefix)
                    && candidate.generated_ids.contains(&event.id);
            }
            if event.metadata["provider_log"] != true {
                return true;
            }
            if matches!(candidate.context.provider.as_str(), "codex" | "pi")
                && !event.metadata["chat_source_ref"].is_string()
            {
                // Keep historical physical envelopes usable. Only the separate
                // qualified relation index may supply display correspondence.
                return true;
            }
            if matches!(
                candidate.context.provider.as_str(),
                "opencode" | "antigravity"
            ) && candidate.verified_ids.contains(&event.id)
            {
                return true;
            }
            capture.provider_log_sources.iter().any(|source| {
                candidate.context.provider_source_key.as_deref()
                    == Some(source.provider_source_key.as_str())
                    && event.metadata["chat_source_epoch"].as_str()
                        == Some(
                            crate::commands::chat_recent_seed::hash(
                                &serde_json::to_vec(&source.native_identity).unwrap_or_default(),
                            )
                            .as_str(),
                        )
                    && event.metadata["chat_source_start"]
                        .as_u64()
                        .zip(event.metadata["chat_source_end"].as_u64())
                        .is_some_and(|(start, end)| {
                            start < end
                                && end <= source.committed_offset
                                && start >= source.unknown_before_offset.unwrap_or(0)
                                && source
                                    .open_disabled_from
                                    .is_none_or(|disabled| end <= disabled)
                                && !source
                                    .disabled_spans
                                    .iter()
                                    .any(|span| start < span.end && end > span.start)
                        })
            })
        });
        let stamp = capture
            .provider_log_sources
            .iter()
            .map(|source| {
                serde_json::json!({
                    "source": source.provider_source_key, "identity": source.native_identity,
                    "offset": source.committed_offset, "policy": source.policy_generation,
                    "disabled": source.disabled_spans, "open_disabled": source.open_disabled_from,
                })
            })
            .collect::<Vec<_>>();
        self.chat_projection.committed(
            candidate,
            crate::commands::chat_recent_seed::hash(
                &serde_json::to_vec(&stamp).map_err(io::Error::other)?,
            ),
        )
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
        let (appended, candidate) = self.append_chat_events_prepared_locked(
            context.clone(),
            events,
            Some(CapturePreparation {
                state: &mut capture_state,
                previous: expected,
                next,
            }),
        )?;
        if let Some(index) = current_index {
            capture_state.provider_log_sources[index] = next.clone();
        } else {
            capture_state.provider_log_sources.push(next.clone());
        }
        let policy_changed = expected.is_none_or(|old| {
            old.native_identity != next.native_identity
                || old.policy_generation != next.policy_generation
                || old.unknown_before_offset != next.unknown_before_offset
                || old.disabled_spans != next.disabled_spans
                || old.open_disabled_from != next.open_disabled_from
        });
        if policy_changed {
            chat_read::policy_barrier(&context.agent_id)?;
        }
        #[cfg(test)]
        if self
            .fail_next_chat_cursor_commit
            .swap(false, Ordering::SeqCst)
        {
            return Err(io::Error::other("injected chat cursor commit failure"));
        }
        write_capture_state(&context.agent_id, &capture_state)?;
        chat_read::publish_policy(&context, next)?;
        #[cfg(test)]
        if self
            .fail_compatibility_stage
            .compare_exchange(4, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Err(io::Error::other(
                "injected compatibility failure after cursor",
            ));
        }
        if let Some(candidate) = candidate {
            self.publish_chat_candidate(candidate)?;
        }
        compatibility_claims::finish(&mut capture_state, &context.agent_id)?;
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

    /// Best-effort receipt for a bounded generated input. Cold, busy and
    /// oversized cases return no receipt; accepted provider input is never
    /// retried because this independent archive operation failed.
    pub(crate) fn append_delivered_input_receipt(
        &self,
        context: ConversationArchiveContext,
        text: &str,
        fence: &chat_read::InputFence,
    ) -> io::Result<Option<wardian_core::models::chat::ChatInputReceipt>> {
        if text.trim().is_empty() || text.len() > 16 * 1024 {
            return Ok(None);
        }
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _agent_guard = match agent_lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(_) => return Err(io::Error::other("chat archive owner poisoned")),
        };
        let mut active = match self.active.try_lock() {
            Ok(active) => active,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(_) => return Err(io::Error::other("chat active owner poisoned")),
        };
        let Some(handle) = active.get_mut(&context.agent_id) else {
            return Ok(None);
        };
        if handle.provider_source_key != context.provider_source_key
            || handle.next_seq == u64::MAX
            || handle.conversation_id != fence.conversation_id
        {
            return Ok(None);
        }
        let conversation_id = handle.conversation_id.clone();
        let seq = handle.next_seq;
        let dir = conversation_dir(&context.agent_id, &conversation_id)?;
        let mut record = narrative_from_delivered_input(&current_rfc3339_millis(), text, None, seq);
        let sources = generated_sources_from_record(&context, &mut record);
        let event = generated_event_from_record(&context, &conversation_id, &mut record);
        // Reserve the actual writer identity before any partial write. An
        // archive failure must not recycle it for a later identical prompt.
        handle.next_seq += 1;
        drop(active);
        self.chat_projection
            .commit_input(&context, &conversation_id, fence, || {
                append_jsonl_record(&dir.join("events.jsonl"), &event)?;
                for source in &sources {
                    append_jsonl_record(&dir.join("sources.jsonl"), source)?;
                }
                append_jsonl_record(&dir.join("conversation.jsonl"), &record)?;
                for file in ["events.jsonl", "sources.jsonl", "conversation.jsonl"] {
                    if file == "sources.jsonl" && sources.is_empty() {
                        continue;
                    }
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(dir.join(file))?
                        .sync_data()?;
                }
                // Derived turns/index summaries are coalesced background repair;
                // this owned immutable display publication seals the generated row.
                let extent = std::fs::metadata(dir.join("events.jsonl"))?.len();
                self.deferred_receipt_summaries
                    .lock()
                    .map_err(|_| io::Error::other("chat maintenance owner poisoned"))?
                    .insert(context.agent_id.clone(), conversation_id.clone());
                Ok((event, extent))
            })
    }

    pub(crate) fn has_deferred_chat_summaries(&self, agent_id: &str) -> bool {
        self.deferred_receipt_summaries
            .lock()
            .is_ok_and(|pending| pending.contains_key(agent_id))
    }

    /// The existing background owner maintains full archive summaries. Normal
    /// reads and submissions never await this legacy derivation work.
    pub(crate) fn flush_deferred_chat_summaries(
        &self,
        context: &ConversationArchiveContext,
    ) -> io::Result<()> {
        let pending = self
            .deferred_receipt_summaries
            .lock()
            .map_err(|_| io::Error::other("chat maintenance owner poisoned"))?
            .get(&context.agent_id)
            .cloned();
        let Some(conversation_id) = pending else {
            return Ok(());
        };
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        let active = lock_active(&self.active)?.get(&context.agent_id).cloned();
        if let Some(handle) = active.filter(|handle| {
            handle.conversation_id == conversation_id
                && handle.provider_source_key == context.provider_source_key
        }) {
            let directory = conversation_dir(&context.agent_id, &conversation_id)?;
            let records = read_jsonl_records(&directory.join("conversation.jsonl"))?;
            let events = read_jsonl_records(&directory.join("events.jsonl"))?;
            repair::rebuild_receipt_summaries(
                &context.agent_id,
                &directory,
                context,
                &handle,
                &records,
                &events,
            )?;
        }
        let mut pending = self
            .deferred_receipt_summaries
            .lock()
            .map_err(|_| io::Error::other("chat maintenance owner poisoned"))?;
        if pending.get(&context.agent_id) == Some(&conversation_id) {
            pending.remove(&context.agent_id);
        }
        Ok(())
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
        let conversation_id = handle.conversation_id.clone();
        let generated_input_bindings =
            chat_read::generated_input_bindings(&all_records, &conversation_id);
        lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
        self.publish_chat_candidate(chat_read::Candidate {
            context: effective_context,
            conversation_id,
            events: all_events,
            committed_output_bytes: std::fs::metadata(&events_path)?.len(),
            verified_ids: HashSet::new(),
            generated_input_bindings,
            generated_ids: all_records
                .iter()
                .flat_map(|record| record.event_refs.iter())
                .cloned()
                .collect(),
            source_epoch: None,
        })?;
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
        drop(active);
        self.chat_projection.invalidate(agent_id)?;
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
        drop(active);
        self.chat_projection.invalidate(agent_id)?;
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

/// Sequence numbers narrow the lookup; full equality still decides whether
/// materialization is needed. Keep duplicate sequences in the same bucket so
/// an inconsistent archive has the same membership semantics as a slice scan.
fn index_records_by_sequence(
    records: &[ConversationNarrativeRecord],
) -> HashMap<u64, Vec<&ConversationNarrativeRecord>> {
    let mut indexed = HashMap::<u64, Vec<&ConversationNarrativeRecord>>::new();
    for record in records {
        indexed.entry(record.seq).or_default().push(record);
    }
    indexed
}

fn record_was_present(
    record: &ConversationNarrativeRecord,
    indexed: &HashMap<u64, Vec<&ConversationNarrativeRecord>>,
) -> bool {
    indexed
        .get(&record.seq)
        .is_some_and(|previous| previous.contains(&record))
}

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
