use std::{
    collections::{HashMap, HashSet},
    io,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, read_jsonl_records, write_json_atomic,
    write_jsonl_atomic, AgentConversationLoggingSetting, ConversationBoundaryReason,
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationSourceRecord, ConversationSpeakerType,
};
use wardian_core::models::chat::AgentChatEvent;

mod records;
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
use turns::derive_turn_records;
use turns::{apply_archive_summary_to_manifest, archive_summary};

#[derive(Debug, Default)]
pub struct ConversationArchiveState {
    #[allow(dead_code)]
    active: Mutex<HashMap<String, ActiveConversationHandle>>,
    agent_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConversationCaptureState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_events_at_or_before: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skip_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skip_event_scopes: Vec<ConversationCaptureEventScope>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConversationCaptureEventScope {
    provider_source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_events_at_or_before: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    event_ids: Vec<String>,
}

impl ConversationCaptureState {
    fn should_skip_event(&self, event: &AgentChatEvent, provider_source_key: Option<&str>) -> bool {
        let legacy_unscoped_match =
            provider_source_key.is_none() && self.skip_event_ids.iter().any(|id| id == &event.id);
        let scoped_match = self.skip_event_scopes.iter().any(|scope| {
            scope.provider_source_key.as_deref() == provider_source_key
                && (scope.event_ids.iter().any(|id| id == &event.id)
                    || scope
                        .skip_events_at_or_before
                        .as_deref()
                        .zip(event.created_at.as_deref())
                        .is_some_and(|(cutoff, created_at)| created_at <= cutoff))
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
        mut context: ConversationArchiveContext,
        events: &[AgentChatEvent],
    ) -> io::Result<usize> {
        if !events
            .iter()
            .any(|event| record_kind_from_chat_event_kind(&event.kind).is_some())
        {
            return Ok(0);
        }

        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
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
        let mut handle =
            active_handle_for_context(&self.active, &context, provider_source_key.clone())?;
        let conversation_dir = conversation_dir(&context.agent_id, &handle.conversation_id)?;
        let effective_context = effective_context_for_handle(&context, &handle, &conversation_dir)?;
        let conversation_path = conversation_dir.join("conversation.jsonl");
        let events_path = conversation_dir.join("events.jsonl");
        let sources_path = conversation_dir.join("sources.jsonl");
        let capture_state = read_capture_state(&context.agent_id)?;
        let mut existing_records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path)?;
        let mut seen_event_ids = existing_records
            .iter()
            .flat_map(|record| record.event_refs.iter().cloned())
            .collect::<HashSet<_>>();
        let mut next_seq = handle.next_seq.max(
            existing_records
                .iter()
                .map(|record| record.seq)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        let mut appended = Vec::new();
        let mut event_records = Vec::new();
        let mut source_records = Vec::new();
        let mut merged_existing_count = 0_usize;

        for event in events {
            if capture_state.should_skip_event(event, provider_source_key.as_deref()) {
                continue;
            }
            if !seen_event_ids.insert(event.id.clone()) {
                continue;
            }
            if let Some(record_index) =
                matching_delivered_input_record_index(&existing_records, event)
            {
                let record_seq = existing_records[record_index].seq;
                let source_record = source_record_from_chat_event(event, record_seq);
                if let Some(source_record) = source_record {
                    if !existing_records[record_index]
                        .source_refs
                        .iter()
                        .any(|source_ref| source_ref == &source_record.source_id)
                    {
                        existing_records[record_index]
                            .source_refs
                            .push(source_record.source_id.clone());
                    }
                    append_jsonl_record(&sources_path, &source_record)?;
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
                let event_record = event_record_for_jsonl(event, &existing_records[record_index]);
                append_jsonl_record(&events_path, &event_record)?;
                merged_existing_count = merged_existing_count.saturating_add(1);
                continue;
            }
            let Some(mut record) = narrative_from_chat_event(event, next_seq) else {
                continue;
            };
            materialize_record_text(&conversation_dir, &mut record)?;
            if let Some(source_record) = source_record_from_chat_event(event, next_seq) {
                record.source_refs = vec![source_record.source_id.clone()];
                source_records.push(Some(source_record));
            } else {
                source_records.push(None);
            }
            event_records.push(event_record_for_jsonl(event, &record));
            next_seq = next_seq.saturating_add(1);
            appended.push(record);
        }

        if appended.is_empty() && merged_existing_count == 0 {
            handle.next_seq = next_seq;
            lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
            return Ok(0);
        }

        if merged_existing_count > 0 {
            write_jsonl_atomic(&conversation_path, &existing_records)?;
        }

        for (index, record) in appended.iter().enumerate() {
            if let Some(event_record) = event_records.get(index) {
                append_jsonl_record(&events_path, event_record)?;
            }
            if let Some(Some(source_record)) = source_records.get(index) {
                append_jsonl_record(&sources_path, source_record)?;
            }
            append_jsonl_record(&conversation_path, record)?;
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
        let all_sources: Vec<ConversationSourceRecord> = read_jsonl_records(&sources_path)?;
        let summary = archive_summary(&all_records, &[], &all_sources);
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
        Ok(appended.len().saturating_add(merged_existing_count))
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
        let existing_records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path)?;
        let next_seq = handle.next_seq.max(
            existing_records
                .iter()
                .map(|record| record.seq)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        let mut record = make_record(next_seq);
        materialize_record_text(&conversation_dir, &mut record)?;
        let generated_event =
            generated_event_from_record(&effective_context, &handle.conversation_id, &mut record);
        let generated_sources = generated_sources_from_record(&effective_context, &mut record);
        append_jsonl_record(&events_path, &generated_event)?;
        for source in &generated_sources {
            append_jsonl_record(&sources_path, source)?;
        }
        append_jsonl_record(&conversation_path, &record)?;

        let first_record = existing_records.first().unwrap_or(&record);
        let all_records = existing_records
            .iter()
            .chain(std::iter::once(&record))
            .cloned()
            .collect::<Vec<_>>();
        let all_sources: Vec<ConversationSourceRecord> = read_jsonl_records(&sources_path)?;
        let summary = archive_summary(&all_records, &[], &all_sources);
        let record_count = all_records.len() as u64;
        let mut manifest = open_manifest(
            &effective_context,
            &handle.conversation_id,
            first_record.at.clone(),
            record.at.clone(),
        );
        apply_archive_summary_to_manifest(&mut manifest, &summary);
        write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
        append_index_upsert(
            &index_path(&context.agent_id)?,
            &index_entry_from_manifest(
                &manifest,
                None,
                excerpt_from_record(first_record),
                excerpt_from_record(&record),
                record_count,
                artifact_count_for_records(all_records.iter()),
            ),
        )?;

        handle.next_seq = next_seq.saturating_add(1);
        lock_active(&self.active)?.insert(context.agent_id.clone(), handle);
        Ok(1)
    }

    pub fn rollover_agent(
        &self,
        agent_id: &str,
        reason: ConversationBoundaryReason,
    ) -> io::Result<Option<String>> {
        let agent_lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _agent_guard = lock_agent_archive(&agent_lock)?;
        let Some(handle) = lock_active(&self.active)?.remove(agent_id) else {
            return Ok(None);
        };
        let conversation_dir = conversation_dir(agent_id, &handle.conversation_id)?;
        close_conversation_dir(agent_id, &handle.conversation_id, &conversation_dir, reason)?;
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
            .remove(agent_id)
            .map(|handle| handle.conversation_id);
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
                            event_ids: Vec::new(),
                        });
                    capture_state.skip_event_scopes.len() - 1
                });
            let scope = &mut capture_state.skip_event_scopes[scope_index];
            scope.skip_events_at_or_before = Some(cutoff);
            let mut seen = scope.event_ids.iter().cloned().collect::<HashSet<_>>();
            for event in events {
                if !event.id.trim().is_empty() && seen.insert(event.id.clone()) {
                    scope.event_ids.push(event.id.clone());
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
                if !event.id.trim().is_empty() && seen.insert(event.id.clone()) {
                    capture_state.skip_event_ids.push(event.id.clone());
                }
            }
        }
        write_capture_state(agent_id, &capture_state)?;
        Ok(removed)
    }

    #[cfg(test)]
    pub fn set_active_for_test(&self, agent_id: &str, handle: ActiveConversationHandle) {
        self.active
            .lock()
            .expect("active conversation lock")
            .insert(agent_id.to_string(), handle);
    }

    #[cfg(test)]
    pub fn active_conversation_id_for_test(&self, agent_id: &str) -> Option<String> {
        self.active
            .lock()
            .expect("active conversation lock")
            .get(agent_id)
            .map(|handle| handle.conversation_id.clone())
    }
}
