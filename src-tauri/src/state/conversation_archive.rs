use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, materialize_text_payload, read_jsonl_records,
    read_latest_index_entries, write_json_atomic, write_jsonl_atomic,
    AgentConversationLoggingSetting, ConversationBoundaryReason, ConversationFormatVersions,
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationProviderNativeRef, ConversationRecordKind,
    ConversationSourceRecord, ConversationSpeakerType, ConversationStatus,
    ConversationTurnFailureSignal, ConversationTurnRecord, ConversationTurnStatus,
    CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};
use wardian_core::paths::{agent_conversation_dir, agent_conversations_dir, agents_dir};

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

pub fn narrative_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationNarrativeRecord> {
    let kind = record_kind_from_chat_event_kind(&event.kind)?;
    let role = event.role.as_ref().map(role_to_string);

    Some(ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: event.turn_id.clone(),
        at: event
            .created_at
            .clone()
            .unwrap_or_else(current_rfc3339_millis),
        kind,
        role,
        speaker_type: event.role.as_ref().map(speaker_type_from_role),
        text: event.text.clone(),
        tool: tool_name_from_chat_event(event),
        status: event.status.as_ref().map(status_to_string),
        summary: event.title.clone(),
        excerpt: None,
        event_refs: vec![event.id.clone()],
        source_refs: event.source.iter().cloned().collect(),
        artifact_refs: Vec::new(),
    })
}

fn source_record_from_chat_event(
    event: &AgentChatEvent,
    seq: u64,
) -> Option<ConversationSourceRecord> {
    let source_kind = event
        .source
        .clone()
        .or_else(|| metadata_source_kind(&event.provider, &event.metadata))?;

    Some(ConversationSourceRecord {
        schema: CONVERSATION_SCHEMA,
        source_id: format!("src_{seq}"),
        provider: event.provider.clone(),
        provider_session_id: metadata_string(&event.metadata, "opencode_session_id")
            .or_else(|| metadata_string(&event.metadata, "provider_session_id")),
        source_kind,
        source_path: event
            .path
            .clone()
            .or_else(|| metadata_string(&event.metadata, "source_path"))
            .or_else(|| metadata_string(&event.metadata, "path"))
            .or_else(|| metadata_string(&event.metadata, "log_path")),
        cursor: metadata_string(&event.metadata, "cursor")
            .or_else(|| event.sequence.map(|sequence| sequence.to_string())),
        offset: metadata_u64(&event.metadata, "offset"),
        row_id: metadata_string(&event.metadata, "part_id"),
        provider_event_type: metadata_string(&event.metadata, "raw_type")
            .or_else(|| metadata_string(&event.metadata, "provider_type")),
        hash: metadata_string(&event.metadata, "hash"),
        artifact_ref: metadata_string(&event.metadata, "artifact_ref"),
    })
}

fn matching_delivered_input_record_index(
    records: &[ConversationNarrativeRecord],
    event: &AgentChatEvent,
) -> Option<usize> {
    if event.kind != AgentChatEventKind::Message
        || event.role.as_ref() != Some(&AgentChatRole::User)
    {
        return None;
    }
    let event_text = event.text.as_deref()?.trim();
    if event_text.is_empty() {
        return None;
    }

    let mut matches = records.iter().enumerate().filter_map(|(index, record)| {
        (record.kind == ConversationRecordKind::Message
            && record.role.as_deref() == Some("user")
            && record.turn_id.is_none()
            && record
                .event_refs
                .iter()
                .all(|event_ref| event_ref.starts_with("generated:"))
            && record
                .text
                .as_deref()
                .is_some_and(|text| text.trim() == event_text))
        .then_some(index)
    });
    let index = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn tool_name_from_chat_event(event: &AgentChatEvent) -> Option<String> {
    metadata_string(&event.metadata, "tool_name")
        .or_else(|| event.title.clone())
        .map(|tool| tool.trim().to_string())
        .filter(|tool| !tool.is_empty())
}

fn generated_event_from_record(
    context: &ConversationArchiveContext,
    conversation_id: &str,
    record: &mut ConversationNarrativeRecord,
) -> AgentChatEvent {
    let event_id = format!("generated:{conversation_id}:{}", record.seq);
    if record.event_refs.is_empty() {
        record.event_refs.push(event_id.clone());
    }

    AgentChatEvent {
        id: event_id,
        session_id: context.agent_id.clone(),
        provider: context.provider.clone(),
        kind: match record.kind {
            ConversationRecordKind::Message => AgentChatEventKind::Message,
            ConversationRecordKind::ToolCall => AgentChatEventKind::ToolCall,
            ConversationRecordKind::ToolResult => AgentChatEventKind::ToolResult,
            ConversationRecordKind::Approval => AgentChatEventKind::Approval,
            ConversationRecordKind::Error => AgentChatEventKind::Error,
            ConversationRecordKind::Lifecycle | ConversationRecordKind::Status => {
                AgentChatEventKind::Status
            }
        },
        role: record.role.as_deref().and_then(agent_role_from_record_role),
        text: record.text.clone(),
        title: record.summary.clone(),
        status: (record.kind == ConversationRecordKind::Lifecycle).then_some(AgentChatStatus::Idle),
        turn_id: context.provider_source_key.clone(),
        source: record.source_refs.first().cloned(),
        command: record.tool.clone(),
        exit_code: None,
        path: None,
        language: None,
        created_at: Some(record.at.clone()),
        sequence: Some(record.seq),
        metadata: serde_json::json!({
            "generated": true,
            "conversation_record_kind": record_kind_to_string(record.kind),
        }),
    }
}

fn generated_sources_from_record(
    _context: &ConversationArchiveContext,
    record: &mut ConversationNarrativeRecord,
) -> Vec<ConversationSourceRecord> {
    let mut sources = Vec::new();
    for source_ref in record.source_refs.clone() {
        if let Some(sender_agent_id) = source_ref.strip_prefix("agent:").map(ToString::to_string) {
            sources.push(ConversationSourceRecord {
                schema: CONVERSATION_SCHEMA,
                source_id: source_ref,
                provider: "wardian".to_string(),
                provider_session_id: Some(sender_agent_id),
                source_kind: "wardian_agent".to_string(),
                source_path: None,
                cursor: Some(record.seq.to_string()),
                offset: None,
                row_id: None,
                provider_event_type: Some("delivered_input".to_string()),
                hash: None,
                artifact_ref: None,
            });
        }
    }

    if record.kind == ConversationRecordKind::Lifecycle {
        let source_id = format!("src_{}", record.seq);
        if !record.source_refs.iter().any(|source| source == &source_id) {
            record.source_refs.push(source_id.clone());
        }
        sources.push(ConversationSourceRecord {
            schema: CONVERSATION_SCHEMA,
            source_id,
            provider: "wardian".to_string(),
            provider_session_id: None,
            source_kind: "wardian_lifecycle".to_string(),
            source_path: None,
            cursor: Some(record.seq.to_string()),
            offset: None,
            row_id: None,
            provider_event_type: record.status.clone(),
            hash: None,
            artifact_ref: None,
        });
    }

    sources
}

fn agent_role_from_record_role(role: &str) -> Option<AgentChatRole> {
    match role {
        "user" => Some(AgentChatRole::User),
        "assistant" => Some(AgentChatRole::Assistant),
        "system" => Some(AgentChatRole::System),
        "tool" => Some(AgentChatRole::Tool),
        _ => None,
    }
}

fn record_kind_to_string(kind: ConversationRecordKind) -> &'static str {
    match kind {
        ConversationRecordKind::Message => "message",
        ConversationRecordKind::ToolCall => "tool_call",
        ConversationRecordKind::ToolResult => "tool_result",
        ConversationRecordKind::Approval => "approval",
        ConversationRecordKind::Error => "error",
        ConversationRecordKind::Lifecycle => "lifecycle",
        ConversationRecordKind::Status => "status",
    }
}

pub fn narrative_from_delivered_input(
    at: &str,
    text: &str,
    sender_agent_id: Option<&str>,
    seq: u64,
) -> ConversationNarrativeRecord {
    ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: None,
        at: at.to_string(),
        kind: ConversationRecordKind::Message,
        role: Some("user".to_string()),
        speaker_type: Some(if sender_agent_id.is_some() {
            ConversationSpeakerType::Agent
        } else {
            ConversationSpeakerType::Unknown
        }),
        text: Some(text.to_string()),
        tool: None,
        status: None,
        summary: None,
        excerpt: None,
        event_refs: Vec::new(),
        source_refs: sender_agent_id
            .map(|sender| vec![format!("agent:{sender}")])
            .unwrap_or_default(),
        artifact_refs: Vec::new(),
    }
}

pub fn lifecycle_record(
    seq: u64,
    reason: ConversationBoundaryReason,
    at: &str,
) -> ConversationNarrativeRecord {
    ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: None,
        at: at.to_string(),
        kind: ConversationRecordKind::Lifecycle,
        role: Some("system".to_string()),
        speaker_type: Some(ConversationSpeakerType::System),
        text: None,
        tool: None,
        status: Some(boundary_reason_to_string(reason)),
        summary: Some(format!(
            "conversation {}",
            boundary_reason_to_string(reason)
        )),
        excerpt: None,
        event_refs: Vec::new(),
        source_refs: Vec::new(),
        artifact_refs: Vec::new(),
    }
}

fn record_kind_from_chat_event_kind(kind: &AgentChatEventKind) -> Option<ConversationRecordKind> {
    match kind {
        AgentChatEventKind::Message => Some(ConversationRecordKind::Message),
        AgentChatEventKind::ToolCall => Some(ConversationRecordKind::ToolCall),
        AgentChatEventKind::ToolResult => Some(ConversationRecordKind::ToolResult),
        AgentChatEventKind::Approval => Some(ConversationRecordKind::Approval),
        AgentChatEventKind::Status => None,
        AgentChatEventKind::Error => Some(ConversationRecordKind::Error),
        AgentChatEventKind::TerminalOutput => None,
    }
}

fn role_to_string(role: &AgentChatRole) -> String {
    match role {
        AgentChatRole::User => "user",
        AgentChatRole::Assistant => "assistant",
        AgentChatRole::System => "system",
        AgentChatRole::Tool => "tool",
    }
    .to_string()
}

fn speaker_type_from_role(role: &AgentChatRole) -> ConversationSpeakerType {
    match role {
        AgentChatRole::User => ConversationSpeakerType::User,
        AgentChatRole::Assistant => ConversationSpeakerType::Assistant,
        AgentChatRole::System => ConversationSpeakerType::System,
        AgentChatRole::Tool => ConversationSpeakerType::Tool,
    }
}

fn status_to_string(status: &wardian_core::models::chat::AgentChatStatus) -> String {
    match status {
        wardian_core::models::chat::AgentChatStatus::Running => "running",
        wardian_core::models::chat::AgentChatStatus::Succeeded => "succeeded",
        wardian_core::models::chat::AgentChatStatus::Failed => "failed",
        wardian_core::models::chat::AgentChatStatus::ActionRequired => "action_required",
        wardian_core::models::chat::AgentChatStatus::Cancelled => "cancelled",
        wardian_core::models::chat::AgentChatStatus::Idle => "idle",
        wardian_core::models::chat::AgentChatStatus::Processing => "processing",
        wardian_core::models::chat::AgentChatStatus::Unknown => "unknown",
    }
    .to_string()
}

fn has_provider_metadata(metadata: &serde_json::Value) -> bool {
    [
        "opencode_session_id",
        "part_id",
        "raw_type",
        "provider_type",
        "cursor",
        "offset",
        "provider_log",
        "provider_source",
        "log_source",
        "source_path",
        "log_path",
        "hash",
        "artifact_ref",
    ]
    .iter()
    .any(|key| metadata.get(*key).is_some())
}

fn metadata_source_kind(provider: &str, metadata: &serde_json::Value) -> Option<String> {
    metadata_string(metadata, "log_source")
        .or_else(|| metadata_string(metadata, "provider_source"))
        .or_else(|| metadata_string(metadata, "provider_type"))
        .or_else(|| metadata_string(metadata, "raw_type"))
        .or_else(|| has_provider_metadata(metadata).then(|| format!("{provider}_metadata")))
}

fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    let value = metadata.get(key)?;
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn metadata_u64(metadata: &serde_json::Value, key: &str) -> Option<u64> {
    let value = metadata.get(key)?;
    value.as_u64().or_else(|| {
        value
            .as_i64()
            .and_then(|number| u64::try_from(number).ok())
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

fn boundary_reason_to_string(reason: ConversationBoundaryReason) -> String {
    match reason {
        ConversationBoundaryReason::Spawn => "spawn",
        ConversationBoundaryReason::ProviderSourceChanged => "provider_source_changed",
        ConversationBoundaryReason::Clear => "clear",
        ConversationBoundaryReason::WorktreeSwitch => "worktree_switch",
        ConversationBoundaryReason::LoggingEnabled => "logging_enabled",
        ConversationBoundaryReason::Shutdown => "shutdown",
    }
    .to_string()
}

fn current_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn lock_active(
    active: &Mutex<HashMap<String, ActiveConversationHandle>>,
) -> io::Result<std::sync::MutexGuard<'_, HashMap<String, ActiveConversationHandle>>> {
    active
        .lock()
        .map_err(|_| io::Error::other("conversation archive state lock poisoned"))
}

fn agent_lock_for(
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

fn lock_agent_archive(lock: &Arc<Mutex<()>>) -> io::Result<std::sync::MutexGuard<'_, ()>> {
    lock.lock()
        .map_err(|_| io::Error::other("conversation archive agent lock poisoned"))
}

fn conversation_dir(agent_id: &str, conversation_id: &str) -> io::Result<std::path::PathBuf> {
    agent_conversation_dir(agent_id, conversation_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe conversation path"))
}

fn index_path(agent_id: &str) -> io::Result<std::path::PathBuf> {
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

fn read_capture_state(agent_id: &str) -> io::Result<ConversationCaptureState> {
    let path = capture_state_path(agent_id)?;
    if !path.exists() {
        return Ok(ConversationCaptureState::default());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(io::Error::other)
}

fn write_capture_state(agent_id: &str, state: &ConversationCaptureState) -> io::Result<()> {
    write_json_atomic(&capture_state_path(agent_id)?, state)
}

fn read_agent_index(agent_id: &str) -> io::Result<Vec<ConversationIndexEntry>> {
    read_latest_index_entries(&index_path(agent_id)?)
}

fn read_all_agent_indexes() -> io::Result<Vec<ConversationIndexEntry>> {
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

fn new_conversation_id(agent_id: &str) -> String {
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

fn provider_from_events(events: &[AgentChatEvent]) -> Option<String> {
    events
        .iter()
        .map(|event| event.provider.trim())
        .find(|provider| !provider.is_empty())
        .map(ToString::to_string)
}

fn provider_source_key_from_events(events: &[AgentChatEvent]) -> Option<String> {
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

fn provider_session_ids_from_events(events: &[AgentChatEvent]) -> Vec<String> {
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

fn active_handle_for_context(
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

fn effective_context_for_handle(
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

fn close_conversation_dir(
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
    let turns = derive_turn_records(&records, &events, &sources);
    write_jsonl_atomic(&conversation_dir.join("turns.jsonl"), &turns)?;
    let summary = archive_summary(&records, &turns, &sources);
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

fn read_manifest(path: &std::path::Path) -> io::Result<Option<ConversationManifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(io::Error::other)
}

fn open_manifest(
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

fn index_entry_from_manifest(
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

#[derive(Debug)]
struct ConversationArchiveSummary {
    record_count: u64,
    turn_count: u64,
    has_turns: bool,
    lifecycle_only: bool,
    provider_native_refs: Vec<ConversationProviderNativeRef>,
}

#[derive(Debug)]
struct TurnAccumulator {
    turn_id: Option<String>,
    records: Vec<ConversationNarrativeRecord>,
    events: Vec<AgentChatEvent>,
    sources: Vec<ConversationSourceRecord>,
}

fn derive_turn_records(
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
    sources: &[ConversationSourceRecord],
) -> Vec<ConversationTurnRecord> {
    let events_by_id = events
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect::<HashMap<_, _>>();
    let sources_by_id = sources
        .iter()
        .map(|source| (source.source_id.as_str(), source))
        .collect::<HashMap<_, _>>();
    let mut turns: Vec<TurnAccumulator> = Vec::new();
    let mut current_unkeyed: Option<usize> = None;

    for record in records
        .iter()
        .filter(|record| record.kind != ConversationRecordKind::Lifecycle)
    {
        let turn_index = if let Some(turn_id) = record.turn_id.as_deref() {
            current_unkeyed = None;
            if turns
                .last()
                .is_some_and(|turn| turn.turn_id.as_deref() == Some(turn_id))
            {
                turns.len() - 1
            } else {
                turns.push(TurnAccumulator {
                    turn_id: Some(turn_id.to_string()),
                    records: Vec::new(),
                    events: Vec::new(),
                    sources: Vec::new(),
                });
                turns.len() - 1
            }
        } else if record.kind == ConversationRecordKind::Message
            && record.role.as_deref() == Some("user")
        {
            turns.push(TurnAccumulator {
                turn_id: None,
                records: Vec::new(),
                events: Vec::new(),
                sources: Vec::new(),
            });
            let index = turns.len() - 1;
            current_unkeyed = Some(index);
            index
        } else if let Some(index) = current_unkeyed {
            index
        } else {
            turns.push(TurnAccumulator {
                turn_id: None,
                records: Vec::new(),
                events: Vec::new(),
                sources: Vec::new(),
            });
            let index = turns.len() - 1;
            current_unkeyed = Some(index);
            index
        };

        let turn = &mut turns[turn_index];
        turn.events.extend(
            record
                .event_refs
                .iter()
                .filter_map(|event_id| events_by_id.get(event_id.as_str()).copied().cloned()),
        );
        turn.sources.extend(
            record
                .source_refs
                .iter()
                .filter_map(|source_id| sources_by_id.get(source_id.as_str()).copied().cloned()),
        );
        turn.records.push(record.clone());
    }

    turns
        .into_iter()
        .map(turn_record_from_accumulator)
        .collect()
}

fn turn_record_from_accumulator(turn: TurnAccumulator) -> ConversationTurnRecord {
    let seq_start = turn
        .records
        .iter()
        .map(|record| record.seq)
        .min()
        .unwrap_or(0);
    let seq_end = turn
        .records
        .iter()
        .map(|record| record.seq)
        .max()
        .unwrap_or(seq_start);
    let user_message = turn.records.iter().find(|record| {
        record.kind == ConversationRecordKind::Message && record.role.as_deref() == Some("user")
    });
    let assistant_message = turn.records.iter().rev().find(|record| {
        record.kind == ConversationRecordKind::Message
            && record.role.as_deref() == Some("assistant")
    });
    let mut tools_used = BTreeMap::new();
    for record in turn
        .records
        .iter()
        .filter(|record| record.kind == ConversationRecordKind::ToolCall)
    {
        *tools_used.entry(tool_name_for_record(record)).or_insert(0) += 1;
    }
    for record in turn
        .records
        .iter()
        .filter(|record| record.kind == ConversationRecordKind::ToolResult)
    {
        let tool = tool_name_for_record(record);
        tools_used.entry(tool).or_insert(1);
    }

    let mut failed_tool_count = 0;
    let mut command_nonzero_count = 0;
    let mut failure_signals = Vec::new();
    let mut files_read = Vec::new();
    let mut files_written = Vec::new();
    let mut external_side_effects = Vec::new();
    let events_by_record_seq = turn
        .records
        .iter()
        .flat_map(|record| {
            record.event_refs.iter().filter_map(|event_id| {
                turn.events
                    .iter()
                    .find(|event| &event.id == event_id)
                    .map(|event| (record.seq, event))
            })
        })
        .collect::<Vec<_>>();
    for (seq, event) in events_by_record_seq {
        if matches!(
            event.kind,
            AgentChatEventKind::ToolCall | AgentChatEventKind::ToolResult
        ) && event.status == Some(AgentChatStatus::Failed)
        {
            failed_tool_count += 1;
            failure_signals.push(ConversationTurnFailureSignal {
                signal: "tool_failed".to_string(),
                seq,
            });
        }
        if event.exit_code.is_some_and(|exit_code| exit_code != 0) {
            command_nonzero_count += 1;
            failure_signals.push(ConversationTurnFailureSignal {
                signal: "command_nonzero_exit".to_string(),
                seq,
            });
        }
        extend_unique(
            &mut files_read,
            metadata_string_array(&event.metadata, "files_read"),
        );
        extend_unique(
            &mut files_written,
            metadata_string_array(&event.metadata, "files_written"),
        );
        extend_unique(
            &mut external_side_effects,
            metadata_string_array(&event.metadata, "external_side_effects"),
        );
    }

    let (status, status_source) = if failed_tool_count > 0 {
        (
            ConversationTurnStatus::Failed,
            Some("tool_failure".to_string()),
        )
    } else if command_nonzero_count > 0 {
        (
            ConversationTurnStatus::Failed,
            Some("command_nonzero_exit".to_string()),
        )
    } else {
        (ConversationTurnStatus::Unknown, None)
    };
    ConversationTurnRecord {
        schema: CONVERSATION_SCHEMA,
        turn_id: turn.turn_id,
        seq_start,
        seq_end,
        user_message_seq: user_message.map(|record| record.seq),
        assistant_message_seq: assistant_message.map(|record| record.seq),
        user_message_text: user_message.and_then(|record| record.text.clone()),
        user_message_excerpt: user_message.and_then(|record| record.excerpt.clone()),
        assistant_message_text: assistant_message.and_then(|record| record.text.clone()),
        assistant_message_excerpt: assistant_message.and_then(|record| record.excerpt.clone()),
        status,
        status_source,
        tools_used,
        failed_tool_count,
        command_nonzero_count,
        files_read,
        files_written,
        external_side_effects,
        failure_signals,
        provider_native_refs: provider_native_refs_from_sources(&turn.sources),
    }
}

fn archive_summary(
    records: &[ConversationNarrativeRecord],
    turns: &[ConversationTurnRecord],
    sources: &[ConversationSourceRecord],
) -> ConversationArchiveSummary {
    let lifecycle_only = !records.is_empty()
        && records
            .iter()
            .all(|record| record.kind == ConversationRecordKind::Lifecycle);
    ConversationArchiveSummary {
        record_count: records.len() as u64,
        turn_count: turns.len() as u64,
        has_turns: !turns.is_empty(),
        lifecycle_only,
        provider_native_refs: provider_native_refs_from_sources(sources),
    }
}

fn apply_archive_summary_to_manifest(
    manifest: &mut ConversationManifest,
    summary: &ConversationArchiveSummary,
) {
    manifest.record_count = summary.record_count;
    manifest.turn_count = summary.turn_count;
    manifest.has_turns = summary.has_turns;
    manifest.lifecycle_only = summary.lifecycle_only;
    manifest.provider_native_refs = summary.provider_native_refs.clone();
}

fn provider_native_refs_from_sources(
    sources: &[ConversationSourceRecord],
) -> Vec<ConversationProviderNativeRef> {
    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    for source in sources {
        if source.provider == "wardian"
            || (source.provider_session_id.is_none() && source.source_path.is_none())
        {
            continue;
        }
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            source.provider,
            source.provider_session_id.as_deref().unwrap_or_default(),
            source.source_kind,
            source.source_path.as_deref().unwrap_or_default()
        );
        if seen.insert(key) {
            refs.push(ConversationProviderNativeRef {
                provider: source.provider.clone(),
                provider_session_id: source.provider_session_id.clone(),
                source_kind: source.source_kind.clone(),
                source_path: source.source_path.clone(),
            });
        }
    }
    refs
}

fn metadata_string_array(metadata: &serde_json::Value, key: &str) -> Vec<String> {
    metadata
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn tool_name_for_record(record: &ConversationNarrativeRecord) -> String {
    record
        .tool
        .as_deref()
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
        .unwrap_or("unknown_tool")
        .to_string()
}

fn extend_unique(values: &mut Vec<String>, next_values: Vec<String>) {
    let mut seen = values.iter().cloned().collect::<HashSet<_>>();
    for value in next_values {
        if seen.insert(value.clone()) {
            values.push(value);
        }
    }
}

fn materialize_record_text(
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

fn event_record_for_jsonl(
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

fn artifact_count_for_records<'a>(
    records: impl Iterator<Item = &'a ConversationNarrativeRecord>,
) -> u64 {
    records
        .map(|record| record.artifact_refs.len() as u64)
        .sum()
}

fn excerpt_from_record(record: &ConversationNarrativeRecord) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::{
        derive_turn_records, effective_conversation_logging, lifecycle_record,
        narrative_from_chat_event, narrative_from_delivered_input, new_conversation_id,
        ActiveConversationHandle, ConversationArchiveContext, ConversationArchiveState,
    };
    use wardian_core::conversations::{
        read_jsonl_records, AgentConversationLoggingSetting, ConversationBoundaryReason,
        ConversationLoggingSetting, ConversationManifest, ConversationNarrativeRecord,
        ConversationRecordKind, ConversationSourceRecord, ConversationSpeakerType,
        ConversationStatus, ConversationTurnRecord, ConversationTurnStatus, CONVERSATION_SCHEMA,
    };
    use wardian_core::models::chat::{
        AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
    };
    use wardian_core::paths::{agent_conversation_dir, agent_conversations_dir};

    #[test]
    fn user_chat_message_converts_to_primary_narrative_record() {
        let event = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::User),
            Some("Please inspect the workspace."),
        );

        let record = narrative_from_chat_event(&event, 42).expect("narrative record");

        assert_eq!(record.schema, CONVERSATION_SCHEMA);
        assert_eq!(record.seq, 42);
        assert_eq!(record.at, "2026-06-15T00:00:00.000Z");
        assert_eq!(record.kind, ConversationRecordKind::Message);
        assert_eq!(record.role.as_deref(), Some("user"));
        assert_eq!(record.speaker_type, Some(ConversationSpeakerType::User));
        assert_eq!(
            record.text.as_deref(),
            Some("Please inspect the workspace.")
        );
        assert_eq!(record.event_refs, vec!["event-1".to_string()]);
    }

    #[test]
    fn terminal_output_is_not_primary_narrative() {
        let event = chat_event(
            "event-terminal",
            AgentChatEventKind::TerminalOutput,
            None,
            Some("raw terminal output"),
        );

        assert!(narrative_from_chat_event(&event, 1).is_none());
    }

    #[test]
    fn status_events_are_not_primary_narrative() {
        let event = chat_event(
            "event-status",
            AgentChatEventKind::Status,
            None,
            Some("Idle"),
        );

        assert!(narrative_from_chat_event(&event, 1).is_none());
    }

    #[test]
    fn delivered_input_becomes_user_narrative_record() {
        let record = narrative_from_delivered_input(
            "2026-06-15T00:00:01.000Z",
            "Please review the patch.",
            Some("source-agent"),
            7,
        );

        assert_eq!(record.schema, CONVERSATION_SCHEMA);
        assert_eq!(record.seq, 7);
        assert_eq!(record.at, "2026-06-15T00:00:01.000Z");
        assert_eq!(record.kind, ConversationRecordKind::Message);
        assert_eq!(record.role.as_deref(), Some("user"));
        assert_eq!(record.speaker_type, Some(ConversationSpeakerType::Agent));
        assert_eq!(record.text.as_deref(), Some("Please review the patch."));
    }

    #[test]
    fn lifecycle_record_captures_clear_reason() {
        let record = lifecycle_record(
            8,
            ConversationBoundaryReason::Clear,
            "2026-06-15T00:00:02.000Z",
        );

        assert_eq!(record.schema, CONVERSATION_SCHEMA);
        assert_eq!(record.seq, 8);
        assert_eq!(record.at, "2026-06-15T00:00:02.000Z");
        assert_eq!(record.kind, ConversationRecordKind::Lifecycle);
        assert_eq!(record.speaker_type, Some(ConversationSpeakerType::System));
        assert_eq!(record.status.as_deref(), Some("clear"));
    }

    #[test]
    fn effective_logging_respects_agent_override_before_global_default() {
        assert_eq!(
            effective_conversation_logging(
                ConversationLoggingSetting::Disabled,
                AgentConversationLoggingSetting::Default,
            ),
            ConversationLoggingSetting::Disabled
        );
        assert_eq!(
            effective_conversation_logging(
                ConversationLoggingSetting::Disabled,
                AgentConversationLoggingSetting::Enabled,
            ),
            ConversationLoggingSetting::Enabled
        );
        assert_eq!(
            effective_conversation_logging(
                ConversationLoggingSetting::Enabled,
                AgentConversationLoggingSetting::Disabled,
            ),
            ConversationLoggingSetting::Disabled
        );
    }

    #[test]
    fn rollover_closes_existing_handle() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive.set_active_for_test(
            "agent-1",
            ActiveConversationHandle {
                conversation_id: "conv_existing_agent_1".to_string(),
                next_seq: 1,
                provider_source_key: None,
            },
        );

        let conversation_id = archive
            .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
            .expect("rollover succeeds");

        assert_eq!(conversation_id.as_deref(), Some("conv_existing_agent_1"));
        assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
    }

    #[test]
    fn discard_agent_drops_active_handle_without_creating_archive_files() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive.set_active_for_test(
            "agent-1",
            ActiveConversationHandle {
                conversation_id: "conv_existing_agent_1".to_string(),
                next_seq: 1,
                provider_source_key: None,
            },
        );

        let conversation_id = archive.discard_agent("agent-1").expect("discard succeeds");

        assert_eq!(conversation_id.as_deref(), Some("conv_existing_agent_1"));
        assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
        assert!(!agent_conversations_dir("agent-1")
            .expect("conversations dir")
            .exists());
    }

    #[test]
    fn append_chat_events_dedupes_repeated_loads() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let event = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Rendered from the transcript."),
        );

        let first_count = archive
            .append_chat_events("agent-1", std::slice::from_ref(&event))
            .expect("first append succeeds");
        let second_count = archive
            .append_chat_events("agent-1", &[event])
            .expect("second append succeeds");

        assert_eq!(first_count, 1);
        assert_eq!(second_count, 0);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event_refs, vec!["event-1".to_string()]);
    }

    #[test]
    fn append_chat_events_writes_source_records_for_provider_metadata() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let mut event = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Rendered from the opencode database."),
        );
        event.provider = "opencode".to_string();
        event.turn_id = Some("message-1".to_string());
        event.source = Some("opencode_db".to_string());
        event.sequence = Some(42);
        event.metadata = serde_json::json!({
            "opencode_session_id": "ses_opencode",
            "part_id": "part_42",
            "raw_type": "text",
            "cursor": "opencode:part_42",
            "sequence": 42,
            "offset": 128
        });

        let appended = archive
            .append_chat_events("agent-1", &[event])
            .expect("append succeeds");

        assert_eq!(appended, 1);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let sources: Vec<ConversationSourceRecord> =
            read_jsonl_records(&conversation_path.join("sources.jsonl"))
                .expect("read source records");

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_id, "src_1");
        assert_eq!(sources[0].provider, "opencode");
        assert_eq!(
            sources[0].provider_session_id.as_deref(),
            Some("ses_opencode")
        );
        assert_eq!(sources[0].source_kind, "opencode_db");
        assert_eq!(sources[0].cursor.as_deref(), Some("opencode:part_42"));
        assert_eq!(sources[0].offset, Some(128));
        assert_eq!(sources[0].row_id.as_deref(), Some("part_42"));
        assert_eq!(sources[0].provider_event_type.as_deref(), Some("text"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_refs, vec!["src_1".to_string()]);
    }

    #[test]
    fn turn_id_does_not_become_provider_session_or_rollover_key() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let mut first = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("First turn."),
        );
        first.provider = "codex".to_string();
        first.turn_id = Some("turn-1".to_string());
        first.source = Some("response_item".to_string());
        let mut second = chat_event(
            "event-2",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Second turn."),
        );
        second.provider = "codex".to_string();
        second.turn_id = Some("turn-2".to_string());
        second.source = Some("response_item".to_string());

        archive
            .append_chat_events("agent-1", &[first])
            .expect("append first turn");
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation");
        archive
            .append_chat_events("agent-1", &[second])
            .expect("append second turn");

        assert_eq!(
            archive
                .active_conversation_id_for_test("agent-1")
                .as_deref(),
            Some(conversation_id.as_str())
        );
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let sources: Vec<ConversationSourceRecord> =
            read_jsonl_records(&conversation_path.join("sources.jsonl"))
                .expect("read source records");
        assert_eq!(sources.len(), 2);
        assert!(sources
            .iter()
            .all(|source| source.provider_session_id.is_none()));
    }

    #[test]
    fn command_string_is_not_used_as_turn_tool_name() {
        let event = AgentChatEvent {
            command: Some("cargo test --workspace".to_string()),
            title: None,
            metadata: serde_json::json!({}),
            ..chat_event(
                "event-tool",
                AgentChatEventKind::ToolCall,
                None,
                Some("Running tests."),
            )
        };

        let record = narrative_from_chat_event(&event, 1).expect("narrative record");

        assert_eq!(record.tool, None);
    }

    #[test]
    fn repeated_turn_id_after_another_keyed_turn_starts_new_span() {
        let records = vec![
            narrative_record_with_turn(1, "turn-1", "First A."),
            narrative_record_with_turn(2, "turn-2", "Second."),
            narrative_record_with_turn(3, "turn-1", "First B."),
        ];

        let turns = derive_turn_records(&records, &[], &[]);

        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(turns[0].seq_start, 1);
        assert_eq!(turns[0].seq_end, 1);
        assert_eq!(turns[2].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(turns[2].seq_start, 3);
        assert_eq!(turns[2].seq_end, 3);
    }

    #[test]
    fn append_chat_events_writes_factual_turns_index() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let mut user = chat_event(
            "event-user",
            AgentChatEventKind::Message,
            Some(AgentChatRole::User),
            Some("Run the tests."),
        );
        user.turn_id = Some("turn-1".to_string());
        user.source = Some("response_item".to_string());
        user.metadata = serde_json::json!({
            "log_path": "<absolute-workspace-path>/codex.jsonl",
            "raw_type": "message"
        });
        let mut tool = chat_event(
            "event-tool",
            AgentChatEventKind::ToolCall,
            None,
            Some("Need test output."),
        );
        tool.turn_id = Some("turn-1".to_string());
        tool.title = Some("shell_command".to_string());
        tool.command = Some("cargo test".to_string());
        tool.status = Some(AgentChatStatus::Running);
        tool.source = Some("response_item".to_string());
        let mut result = chat_event(
            "event-result",
            AgentChatEventKind::ToolResult,
            Some(AgentChatRole::Tool),
            Some("test failed"),
        );
        result.turn_id = Some("turn-1".to_string());
        result.title = Some("shell_command".to_string());
        result.status = Some(AgentChatStatus::Failed);
        result.exit_code = Some(101);
        result.source = Some("response_item".to_string());
        let mut assistant = chat_event(
            "event-assistant",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("The test failed."),
        );
        assistant.turn_id = Some("turn-1".to_string());
        assistant.source = Some("response_item".to_string());

        archive
            .append_chat_events_with_context(
                archive_context("session-one"),
                &[user, tool, result, assistant],
            )
            .expect("append events");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        archive
            .rollover_agent("agent-1", ConversationBoundaryReason::Shutdown)
            .expect("close conversation");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let turns: Vec<ConversationTurnRecord> =
            read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");
        let manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read manifest");

        assert_eq!(records[0].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(turns[0].seq_start, 1);
        assert_eq!(turns[0].seq_end, 4);
        assert_eq!(turns[0].user_message_seq, Some(1));
        assert_eq!(turns[0].assistant_message_seq, Some(4));
        assert_eq!(
            turns[0].user_message_text.as_deref(),
            Some("Run the tests.")
        );
        assert_eq!(
            turns[0].assistant_message_text.as_deref(),
            Some("The test failed.")
        );
        assert_eq!(turns[0].status, ConversationTurnStatus::Failed);
        assert_eq!(turns[0].status_source.as_deref(), Some("tool_failure"));
        assert_eq!(turns[0].tools_used.get("shell_command"), Some(&1));
        assert_eq!(turns[0].failed_tool_count, 1);
        assert_eq!(turns[0].command_nonzero_count, 1);
        assert!(turns[0].files_read.is_empty());
        assert!(turns[0].files_written.is_empty());
        assert!(turns[0].external_side_effects.is_empty());
        assert_eq!(turns[0].failure_signals[0].signal, "tool_failed");
        assert_eq!(turns[0].failure_signals[0].seq, 3);
        assert_eq!(manifest.record_count, 4);
        assert_eq!(manifest.turn_count, 1);
        assert!(manifest.has_turns);
        assert!(!manifest.lifecycle_only);

        let json = serde_json::to_value(&turns[0]).unwrap();
        assert!(json.get("capture_quality").is_none());
        assert!(json.get("notes_for_evolver").is_none());
        assert!(json.get("user_correction").is_none());
        let manifest_json = serde_json::to_value(&manifest).unwrap();
        assert!(manifest_json.get("capture_quality").is_none());
    }

    #[test]
    fn append_chat_events_rolls_over_when_provider_source_changes() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let first = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("First provider source."),
        );
        let second = chat_event(
            "event-2",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Second provider source."),
        );

        archive
            .append_chat_events_with_context(archive_context("session-one"), &[first])
            .expect("append first source");
        let first_conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("first conversation");
        archive
            .append_chat_events_with_context(archive_context("session-two"), &[second])
            .expect("append second source");
        let second_conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("second conversation");

        assert_ne!(first_conversation_id, second_conversation_id);
        let first_manifest_path = agent_conversation_dir("agent-1", &first_conversation_id)
            .expect("first conversation dir")
            .join("manifest.json");
        let first_manifest: ConversationManifest =
            serde_json::from_str(&std::fs::read_to_string(first_manifest_path).unwrap())
                .expect("read first manifest");
        assert_eq!(first_manifest.status, ConversationStatus::Interrupted);
        assert_eq!(
            first_manifest.boundary_reason,
            ConversationBoundaryReason::ProviderSourceChanged
        );
        assert_eq!(
            first_manifest.provider_session_ids,
            vec!["session-one".to_string()]
        );
        assert_eq!(
            first_manifest.provider_source_key.as_deref(),
            Some("codex:session:session-one")
        );
    }

    #[test]
    fn append_chat_events_hydrates_open_conversation_for_same_provider_source() {
        let (_guard, _temp) = isolated_home();
        let first_archive = ConversationArchiveState::default();
        let first = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Before restart."),
        );
        first_archive
            .append_chat_events_with_context(archive_context("session-one"), &[first])
            .expect("append before restart");
        let conversation_id = first_archive
            .active_conversation_id_for_test("agent-1")
            .expect("conversation id before restart");
        let second_archive = ConversationArchiveState::default();
        let second = chat_event(
            "event-2",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("After restart."),
        );

        second_archive
            .append_chat_events_with_context(archive_context("session-one"), &[second])
            .expect("append after restart");

        assert_eq!(
            second_archive.active_conversation_id_for_test("agent-1"),
            Some(conversation_id.clone())
        );
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 2);
    }

    #[test]
    fn append_chat_events_adopts_late_provider_source_key_for_existing_input() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let input_context = ConversationArchiveContext {
            provider_source_key: None,
            provider_session_ids: Vec::new(),
            ..archive_context("session-one")
        };
        archive
            .append_delivered_input_with_context(input_context, "User prompt before source.", None)
            .expect("append delivered input");
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("input conversation id");
        let response = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Assistant response after source discovery."),
        );

        archive
            .append_chat_events_with_context(archive_context("session-one"), &[response])
            .expect("append response");

        assert_eq!(
            archive.active_conversation_id_for_test("agent-1"),
            Some(conversation_id.clone())
        );
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read manifest");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 2);
        assert_eq!(
            manifest.provider_source_key.as_deref(),
            Some("codex:session:session-one")
        );
    }

    #[test]
    fn append_preserves_adopted_provider_source_key_across_unkeyed_append_and_restart() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let unkeyed_context = ConversationArchiveContext {
            provider_source_key: None,
            provider_session_ids: Vec::new(),
            ..archive_context("session-one")
        };
        archive
            .append_delivered_input_with_context(
                unkeyed_context.clone(),
                "User prompt before source.",
                None,
            )
            .expect("append unkeyed delivered input");
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("input conversation id");
        let response = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Assistant response after source discovery."),
        );
        archive
            .append_chat_events_with_context(archive_context("session-one"), &[response])
            .expect("append keyed response");
        archive
            .append_delivered_input_with_context(
                unkeyed_context,
                "Follow-up prompt without source metadata.",
                None,
            )
            .expect("append later unkeyed delivered input");

        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read manifest");
        assert_eq!(
            manifest.provider_source_key.as_deref(),
            Some("codex:session:session-one")
        );
        assert_eq!(manifest.provider_session_ids, vec!["session-one"]);

        let second_archive = ConversationArchiveState::default();
        let next_response = chat_event(
            "event-2",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Different provider session response."),
        );
        second_archive
            .append_chat_events_with_context(archive_context("session-two"), &[next_response])
            .expect("append different session after restart");
        let next_conversation_id = second_archive
            .active_conversation_id_for_test("agent-1")
            .expect("next active conversation id");

        assert_ne!(next_conversation_id, conversation_id);
        let old_manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read old manifest");
        assert_eq!(old_manifest.status, ConversationStatus::Interrupted);
        assert_eq!(
            old_manifest.boundary_reason,
            ConversationBoundaryReason::ProviderSourceChanged
        );
    }

    #[test]
    fn append_chat_events_writes_event_records_for_completed_provider_events() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let event = chat_event(
            "event-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some("Persist this provider event."),
        );

        let appended = archive
            .append_chat_events("agent-1", std::slice::from_ref(&event))
            .expect("append succeeds");

        assert_eq!(appended, 1);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let events: Vec<AgentChatEvent> =
            read_jsonl_records(&conversation_path.join("events.jsonl"))
                .expect("read event records");

        assert_eq!(events, vec![event]);
    }

    #[test]
    fn append_chat_events_materializes_large_text_payloads() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let large_text =
            "x".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);
        let event = chat_event(
            "event-large",
            AgentChatEventKind::ToolResult,
            Some(AgentChatRole::Tool),
            Some(&large_text),
        );

        let appended = archive
            .append_chat_events("agent-1", &[event])
            .expect("append succeeds");

        assert_eq!(appended, 1);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let events: Vec<AgentChatEvent> =
            read_jsonl_records(&conversation_path.join("events.jsonl"))
                .expect("read event records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, None);
        assert!(records[0]
            .excerpt
            .as_deref()
            .is_some_and(|excerpt| large_text.starts_with(excerpt)));
        assert_eq!(
            records[0].artifact_refs,
            vec!["event-large-0001.txt".to_string()]
        );
        assert_eq!(
            std::fs::read_to_string(
                conversation_path
                    .join("artifacts")
                    .join("event-large-0001.txt")
            )
            .expect("read artifact"),
            large_text
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, None);
        assert!(events[0].metadata["text_excerpt"]
            .as_str()
            .is_some_and(|excerpt| large_text.starts_with(excerpt)));
        assert_eq!(
            events[0].metadata["text_artifact_refs"][0],
            "event-large-0001.txt"
        );
    }

    #[test]
    fn append_delivered_input_materializes_large_text_payloads() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let large_text =
            "y".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);

        let appended = archive
            .append_delivered_input("agent-1", &large_text, None)
            .expect("append succeeds");

        assert_eq!(appended, 1);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, None);
        assert!(records[0]
            .excerpt
            .as_deref()
            .is_some_and(|excerpt| large_text.starts_with(excerpt)));
        assert_eq!(
            records[0].artifact_refs,
            vec!["delivered-input-1-0001.txt".to_string()]
        );
        assert_eq!(
            std::fs::read_to_string(
                conversation_path
                    .join("artifacts")
                    .join("delivered-input-1-0001.txt")
            )
            .expect("read artifact"),
            large_text
        );
    }

    #[test]
    fn append_delivered_input_appends_to_active_conversation_with_monotonic_seq() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();

        let first = archive
            .append_delivered_input("agent-1", "First live prompt.", Some("source-agent"))
            .expect("first append succeeds");
        let second = archive
            .append_delivered_input("agent-1", "Second live prompt.", None)
            .expect("second append succeeds");

        assert_eq!(first, 1);
        assert_eq!(second, 1);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 2);
        assert_eq!(
            records[0].speaker_type,
            Some(ConversationSpeakerType::Agent)
        );
        assert_eq!(
            records[1].speaker_type,
            Some(ConversationSpeakerType::Unknown)
        );
    }

    #[test]
    fn provider_user_message_does_not_duplicate_delivered_input_prompt() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input_with_context(
                archive_context("session-one"),
                "Run the focused tests.",
                None,
            )
            .expect("append delivered input");
        let mut provider_event = chat_event(
            "provider-user-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::User),
            Some("Run the focused tests."),
        );
        provider_event.provider = "codex".to_string();
        provider_event.turn_id = Some("turn-1".to_string());
        provider_event.source = Some("response_item".to_string());
        provider_event.metadata = serde_json::json!({
            "provider_session_id": "session-one",
            "raw_type": "message"
        });

        archive
            .append_chat_events_with_context(archive_context("session-one"), &[provider_event])
            .expect("append provider event");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let sources: Vec<ConversationSourceRecord> =
            read_jsonl_records(&conversation_path.join("sources.jsonl"))
                .expect("read source records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text.as_deref(), Some("Run the focused tests."));
        assert_eq!(records[0].event_refs.len(), 2);
        assert!(records[0]
            .event_refs
            .iter()
            .any(|event_ref| event_ref.starts_with("generated:")));
        assert!(records[0]
            .event_refs
            .iter()
            .any(|event_ref| event_ref == "provider-user-1"));
        assert_eq!(records[0].source_refs, vec!["src_1".to_string()]);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].provider, "codex");
    }

    #[test]
    fn repeated_same_text_delivered_inputs_are_not_ambiguously_merged() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input_with_context(archive_context("session-one"), "Again.", None)
            .expect("append first delivered input");
        archive
            .append_delivered_input_with_context(archive_context("session-one"), "Again.", None)
            .expect("append second delivered input");
        let mut provider_event = chat_event(
            "provider-user-1",
            AgentChatEventKind::Message,
            Some(AgentChatRole::User),
            Some("Again."),
        );
        provider_event.provider = "codex".to_string();
        provider_event.turn_id = Some("turn-1".to_string());
        provider_event.source = Some("response_item".to_string());
        provider_event.metadata = serde_json::json!({
            "provider_session_id": "session-one",
            "raw_type": "message"
        });

        archive
            .append_chat_events_with_context(archive_context("session-one"), &[provider_event])
            .expect("append provider event");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");

        assert_eq!(records.len(), 3);
        assert!(records[0]
            .event_refs
            .iter()
            .all(|event_ref| event_ref.starts_with("generated:")));
        assert!(records[1]
            .event_refs
            .iter()
            .all(|event_ref| event_ref.starts_with("generated:")));
        assert_eq!(records[2].event_refs, vec!["provider-user-1".to_string()]);
        assert_eq!(records[2].turn_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn append_empty_delivered_input_does_not_create_conversation_files() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();

        let appended = archive
            .append_delivered_input("agent-1", "   ", Some("source-agent"))
            .expect("append succeeds");

        assert_eq!(appended, 0);
        assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
        assert!(!agent_conversations_dir("agent-1")
            .expect("conversations dir")
            .exists());
    }

    #[test]
    fn append_lifecycle_boundary_uses_existing_active_conversation() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input("agent-1", "Before clear.", None)
            .expect("append input");

        let appended = archive
            .append_lifecycle_boundary("agent-1", ConversationBoundaryReason::Clear)
            .expect("append lifecycle");

        assert_eq!(appended, 1);
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].seq, 2);
        assert_eq!(records[1].kind, ConversationRecordKind::Lifecycle);
        assert_eq!(records[1].status.as_deref(), Some("clear"));
    }

    #[test]
    fn append_lifecycle_boundary_after_restart_hydrates_open_conversation_and_preserves_manifest_metadata(
    ) {
        let (_guard, _temp) = isolated_home();
        let first_archive = ConversationArchiveState::default();
        first_archive
            .append_chat_events_with_context(
                archive_context("session-one"),
                &[chat_event(
                    "event-1",
                    AgentChatEventKind::Message,
                    Some(AgentChatRole::Assistant),
                    Some("Before clear."),
                )],
            )
            .expect("append provider event");
        let conversation_id = first_archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");

        let second_archive = ConversationArchiveState::default();
        second_archive
            .append_lifecycle_boundary("agent-1", ConversationBoundaryReason::Clear)
            .expect("append lifecycle after restart");

        assert_eq!(
            second_archive.active_conversation_id_for_test("agent-1"),
            Some(conversation_id.clone())
        );
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read manifest");

        assert_eq!(records.len(), 2);
        assert_eq!(records[1].kind, ConversationRecordKind::Lifecycle);
        assert_eq!(manifest.provider, "codex");
        assert_eq!(manifest.agent_name, "CoderOne");
        assert_eq!(manifest.agent_class, "Coder");
        assert_eq!(manifest.workspace, "<absolute-workspace-path>");
        assert_eq!(
            manifest.provider_source_key.as_deref(),
            Some("codex:session:session-one")
        );
    }

    #[test]
    fn lifecycle_boundary_with_context_does_not_create_unknown_stub() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let context = ConversationArchiveContext {
            agent_id: "agent-1".to_string(),
            agent_name: "CoderOne".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["session-one".to_string()],
            provider_source_key: Some("codex:session:session-one".to_string()),
        };

        archive
            .append_lifecycle_boundary_with_context(context, ConversationBoundaryReason::Clear)
            .expect("append contextual lifecycle");
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        archive
            .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
            .expect("close conversation");

        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read manifest");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let turns: Vec<ConversationTurnRecord> =
            read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");

        assert_eq!(manifest.agent_name, "CoderOne");
        assert_eq!(manifest.agent_class, "Coder");
        assert_eq!(manifest.workspace, "<absolute-workspace-path>");
        assert_eq!(manifest.provider, "codex");
        assert_eq!(
            manifest.provider_session_ids,
            vec!["session-one".to_string()]
        );
        assert_eq!(
            manifest.provider_source_key.as_deref(),
            Some("codex:session:session-one")
        );
        assert_eq!(manifest.record_count, 1);
        assert_eq!(manifest.turn_count, 0);
        assert!(!manifest.has_turns);
        assert!(manifest.lifecycle_only);
        let manifest_json = serde_json::to_value(&manifest).unwrap();
        assert!(manifest_json.get("capture_quality").is_none());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, ConversationRecordKind::Lifecycle);
        assert!(turns.is_empty());
    }

    #[test]
    fn lifecycle_source_does_not_create_provider_native_ref() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();

        archive
            .append_lifecycle_boundary_with_context(
                archive_context("session-one"),
                ConversationBoundaryReason::Clear,
            )
            .expect("append lifecycle");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let manifest: ConversationManifest = serde_json::from_str(
            &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
        )
        .expect("read manifest");
        let sources: Vec<ConversationSourceRecord> =
            read_jsonl_records(&conversation_path.join("sources.jsonl"))
                .expect("read source records");

        assert!(sources
            .iter()
            .any(|source| source.source_kind == "wardian_lifecycle"));
        assert!(manifest.provider_native_refs.is_empty());
    }

    #[test]
    fn discarded_logging_state_skips_disabled_transcript_events_when_reenabled() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_chat_events_with_context(
                archive_context("session-one"),
                &[chat_event_at(
                    "event-a",
                    "2026-01-01T00:00:00.000Z",
                    "Before disabled.",
                )],
            )
            .expect("append enabled event");
        archive
            .discard_agent_with_context(archive_context("session-one"), &[])
            .expect("discard active capture state");

        let disabled_event =
            chat_event_at("event-b", "2026-01-01T00:00:01.000Z", "While disabled.");
        let enabled_event =
            chat_event_at("event-c", "2999-01-01T00:00:00.000Z", "After re-enabled.");
        archive
            .append_chat_events_with_context(
                archive_context("session-one"),
                &[disabled_event, enabled_event],
            )
            .expect("append after re-enabled");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let texts = records
            .iter()
            .filter_map(|record| record.text.as_deref())
            .collect::<Vec<_>>();

        assert!(texts.contains(&"Before disabled."));
        assert!(!texts.contains(&"While disabled."));
        assert!(texts.contains(&"After re-enabled."));
    }

    #[test]
    fn disabled_capture_state_skips_observed_events_without_timestamps_when_reenabled() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_chat_events_with_context(
                archive_context("session-one"),
                &[chat_event_at(
                    "event-a",
                    "2026-01-01T00:00:00.000Z",
                    "Before disabled.",
                )],
            )
            .expect("append enabled event");

        let disabled_event = chat_event_without_created_at("event-b", "While disabled.");
        archive
            .discard_agent_with_context(
                archive_context("session-one"),
                std::slice::from_ref(&disabled_event),
            )
            .expect("discard active capture state with observed event");
        let enabled_event = chat_event_without_created_at("event-c", "After re-enabled.");
        archive
            .append_chat_events_with_context(
                archive_context("session-one"),
                &[disabled_event, enabled_event],
            )
            .expect("append after re-enabled");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let texts = records
            .iter()
            .filter_map(|record| record.text.as_deref())
            .collect::<Vec<_>>();

        assert!(texts.contains(&"Before disabled."));
        assert!(!texts.contains(&"While disabled."));
        assert!(texts.contains(&"After re-enabled."));
    }

    #[test]
    fn disabled_capture_state_does_not_skip_reused_event_ids_from_new_provider_source() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let disabled_event = chat_event_without_created_at("agent-1:1", "While disabled.");
        archive
            .discard_agent_with_context(
                archive_context("session-one"),
                std::slice::from_ref(&disabled_event),
            )
            .expect("discard disabled event for session one");

        let reused_id_event = chat_event_without_created_at("agent-1:1", "Fresh session event.");
        archive
            .append_chat_events_with_context(archive_context("session-two"), &[reused_id_event])
            .expect("append reused id from new source");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");

        assert!(records
            .iter()
            .any(|record| record.text.as_deref() == Some("Fresh session event.")));
        assert!(!records
            .iter()
            .any(|record| record.text.as_deref() == Some("While disabled.")));
    }

    #[test]
    fn disabled_capture_state_does_not_skip_timestamped_events_from_new_provider_source() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let disabled_event =
            chat_event_at("agent-1:1", "2026-01-01T00:00:00.000Z", "While disabled.");
        archive
            .discard_agent_with_context(
                archive_context("session-one"),
                std::slice::from_ref(&disabled_event),
            )
            .expect("discard disabled event for session one");

        let reused_id_event = chat_event_at(
            "agent-1:1",
            "2026-01-01T00:00:00.000Z",
            "Fresh session timestamped event.",
        );
        archive
            .append_chat_events_with_context(archive_context("session-two"), &[reused_id_event])
            .expect("append reused timestamped id from new source");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");

        assert!(records
            .iter()
            .any(|record| record.text.as_deref() == Some("Fresh session timestamped event.")));
        assert!(!records
            .iter()
            .any(|record| record.text.as_deref() == Some("While disabled.")));
    }

    #[test]
    fn conversation_ids_include_collision_resistant_nonce() {
        let id = new_conversation_id("agent-1");
        let has_uuid_nonce = id
            .split('_')
            .any(|part| part.len() == 32 && part.chars().all(|ch| ch.is_ascii_hexdigit()));

        assert!(
            has_uuid_nonce,
            "conversation id should include a UUID nonce: {id}"
        );
    }

    #[test]
    fn generated_records_write_event_and_source_records() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input("agent-1", "Prompt from peer.", Some("source-agent"))
            .expect("append delivered input");
        archive
            .append_lifecycle_boundary("agent-1", ConversationBoundaryReason::Clear)
            .expect("append lifecycle boundary");

        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&conversation_path.join("conversation.jsonl"))
                .expect("read narrative records");
        let events: Vec<AgentChatEvent> =
            read_jsonl_records(&conversation_path.join("events.jsonl"))
                .expect("read event records");
        let sources: Vec<ConversationSourceRecord> =
            read_jsonl_records(&conversation_path.join("sources.jsonl"))
                .expect("read source records");

        assert_eq!(events.len(), 2);
        assert!(records.iter().all(|record| !record.event_refs.is_empty()));
        assert!(records.iter().all(|record| !record.source_refs.is_empty()));
        assert!(sources
            .iter()
            .any(|source| source.source_kind == "wardian_agent"
                && source.provider_session_id.as_deref() == Some("source-agent")));
        assert!(sources
            .iter()
            .any(|source| source.source_kind == "wardian_lifecycle"));
    }

    #[test]
    fn list_conversation_archive_reads_requested_agent_only() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input("agent-1", "Agent one prompt.", None)
            .expect("append agent one");
        archive
            .append_delivered_input("agent-2", "Agent two prompt.", None)
            .expect("append agent two");

        let entries = archive
            .list(Some("agent-1"), false)
            .expect("list agent conversations");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].agent_id, "agent-1");
        assert!(entries[0].path.starts_with("agents/agent-1/conversations/"));
    }

    #[test]
    fn list_conversation_archive_scope_all_scans_agent_owned_indexes() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input("agent-1", "Agent one prompt.", None)
            .expect("append agent one");
        archive
            .append_delivered_input("agent-2", "Agent two prompt.", None)
            .expect("append agent two");

        let entries = archive
            .list(None, true)
            .expect("list all agent conversations");
        let agent_ids = entries
            .iter()
            .map(|entry| entry.agent_id.as_str())
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(entries.len(), 2);
        assert!(agent_ids.contains("agent-1"));
        assert!(agent_ids.contains("agent-2"));
    }

    #[test]
    fn list_conversation_archive_without_scope_all_uses_current_agent_env() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input("agent-1", "Agent one prompt.", None)
            .expect("append agent one");
        archive
            .append_delivered_input("agent-2", "Agent two prompt.", None)
            .expect("append agent two");
        std::env::set_var("WARDIAN_SESSION_ID", "agent-2");

        let entries = archive
            .list(None, false)
            .expect("list current agent conversations");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].agent_id, "agent-2");
        std::env::remove_var("WARDIAN_SESSION_ID");
    }

    #[test]
    fn list_conversation_archive_without_agent_or_scope_all_requires_current_agent() {
        let (_guard, _temp) = isolated_home();
        std::env::remove_var("WARDIAN_SESSION_ID");
        let archive = ConversationArchiveState::default();

        let error = archive.list(None, false).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("WARDIAN_SESSION_ID"));
    }

    #[test]
    fn show_conversation_archive_reads_manifest_and_records_from_agent_owned_path() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input("agent-1", "Show this prompt.", None)
            .expect("append prompt");
        let conversation_id = archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation");

        let (manifest, records) = archive
            .show(&conversation_id)
            .expect("show conversation archive");

        assert_eq!(manifest.agent_id, "agent-1");
        assert_eq!(manifest.conversation_id, conversation_id);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text.as_deref(), Some("Show this prompt."));
    }

    #[test]
    fn terminal_output_events_are_skipped_and_do_not_create_conversation_files() {
        let (_guard, _temp) = isolated_home();
        let archive = ConversationArchiveState::default();
        let event = chat_event(
            "event-terminal",
            AgentChatEventKind::TerminalOutput,
            None,
            Some("raw terminal repaint"),
        );

        let appended = archive
            .append_chat_events("agent-1", &[event])
            .expect("append succeeds");

        assert_eq!(appended, 0);
        assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
        assert!(!agent_conversations_dir("agent-1")
            .expect("conversations dir")
            .exists());
    }

    fn chat_event(
        id: &str,
        kind: AgentChatEventKind,
        role: Option<AgentChatRole>,
        text: Option<&str>,
    ) -> AgentChatEvent {
        AgentChatEvent {
            id: id.to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind,
            role,
            text: text.map(ToString::to_string),
            title: None,
            status: None,
            turn_id: None,
            source: None,
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: Some("2026-06-15T00:00:00.000Z".to_string()),
            sequence: None,
            metadata: serde_json::json!({}),
        }
    }

    fn chat_event_at(id: &str, created_at: &str, text: &str) -> AgentChatEvent {
        AgentChatEvent {
            created_at: Some(created_at.to_string()),
            ..chat_event(
                id,
                AgentChatEventKind::Message,
                Some(AgentChatRole::Assistant),
                Some(text),
            )
        }
    }

    fn chat_event_without_created_at(id: &str, text: &str) -> AgentChatEvent {
        AgentChatEvent {
            created_at: None,
            ..chat_event(
                id,
                AgentChatEventKind::Message,
                Some(AgentChatRole::Assistant),
                Some(text),
            )
        }
    }

    fn narrative_record_with_turn(
        seq: u64,
        turn_id: &str,
        text: &str,
    ) -> ConversationNarrativeRecord {
        ConversationNarrativeRecord {
            schema: CONVERSATION_SCHEMA,
            seq,
            turn_id: Some(turn_id.to_string()),
            at: "2026-06-15T00:00:00.000Z".to_string(),
            kind: ConversationRecordKind::Message,
            role: Some("assistant".to_string()),
            speaker_type: Some(ConversationSpeakerType::Assistant),
            text: Some(text.to_string()),
            tool: None,
            status: None,
            summary: None,
            excerpt: None,
            event_refs: Vec::new(),
            source_refs: Vec::new(),
            artifact_refs: Vec::new(),
        }
    }

    fn archive_context(provider_session_id: &str) -> ConversationArchiveContext {
        ConversationArchiveContext {
            agent_id: "agent-1".to_string(),
            agent_name: "CoderOne".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec![provider_session_id.to_string()],
            provider_source_key: Some(format!("codex:session:{provider_session_id}")),
        }
    }

    fn isolated_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        (guard, temp)
    }
}
