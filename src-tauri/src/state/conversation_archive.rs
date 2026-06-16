use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Mutex,
};

use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, materialize_text_payload, read_jsonl_records,
    read_latest_index_entries, write_json_atomic, AgentConversationLoggingSetting,
    ConversationBoundaryReason, ConversationFormatVersions, ConversationIndexEntry,
    ConversationLoggingSetting, ConversationManifest, ConversationNarrativeRecord,
    ConversationRecordKind, ConversationSourceRecord, ConversationSpeakerType, ConversationStatus,
    CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};
use wardian_core::paths::{agent_conversation_dir, agent_conversations_dir, agents_dir};

#[derive(Debug, Default)]
pub struct ConversationArchiveState {
    #[allow(dead_code)]
    active: Mutex<HashMap<String, ActiveConversationHandle>>,
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

        let mut active = lock_active(&self.active)?;
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
        let mut handle = active_handle_for_context(&mut active, &context, provider_source_key)?;
        let conversation_dir = conversation_dir(&context.agent_id, &handle.conversation_id)?;
        let conversation_path = conversation_dir.join("conversation.jsonl");
        let events_path = conversation_dir.join("events.jsonl");
        let sources_path = conversation_dir.join("sources.jsonl");
        let existing_records: Vec<ConversationNarrativeRecord> =
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

        for event in events {
            if !seen_event_ids.insert(event.id.clone()) {
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
            event_records.push(event.clone());
            next_seq = next_seq.saturating_add(1);
            appended.push(record);
        }

        if appended.is_empty() {
            handle.next_seq = next_seq;
            active.insert(context.agent_id.clone(), handle);
            return Ok(0);
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
        let record_count = (existing_records.len() + appended.len()) as u64;
        let manifest = open_manifest(
            &context,
            &handle.conversation_id,
            first_record.at.clone(),
            last_record.at.clone(),
        );
        write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
        append_index_upsert(
            &index_path(&context.agent_id)?,
            &index_entry_from_manifest(
                &manifest,
                None,
                excerpt_from_record(first_record),
                excerpt_from_record(last_record),
                record_count,
                artifact_count_for_records(existing_records.iter().chain(appended.iter())),
            ),
        )?;

        handle.next_seq = next_seq;
        active.insert(context.agent_id.clone(), handle);
        Ok(appended.len())
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
        let mut active = lock_active(&self.active)?;
        let provider_source_key = context.provider_source_key.clone();
        if context.provider_source_key.is_none() {
            context.provider_source_key = provider_source_key.clone();
        }
        let mut handle = active_handle_for_context(&mut active, &context, provider_source_key)?;
        let conversation_dir = conversation_dir(&context.agent_id, &handle.conversation_id)?;
        let conversation_path = conversation_dir.join("conversation.jsonl");
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
        append_jsonl_record(&conversation_path, &record)?;

        let first_record = existing_records.first().unwrap_or(&record);
        let record_count = (existing_records.len() + 1) as u64;
        let manifest = open_manifest(
            &context,
            &handle.conversation_id,
            first_record.at.clone(),
            record.at.clone(),
        );
        write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
        append_index_upsert(
            &index_path(&context.agent_id)?,
            &index_entry_from_manifest(
                &manifest,
                None,
                excerpt_from_record(first_record),
                excerpt_from_record(&record),
                record_count,
                artifact_count_for_records(existing_records.iter().chain(std::iter::once(&record))),
            ),
        )?;

        handle.next_seq = next_seq.saturating_add(1);
        active.insert(context.agent_id.clone(), handle);
        Ok(1)
    }

    pub fn rollover_agent(
        &self,
        agent_id: &str,
        reason: ConversationBoundaryReason,
    ) -> io::Result<Option<String>> {
        let mut active = lock_active(&self.active)?;
        let Some(handle) = active.remove(agent_id) else {
            return Ok(None);
        };
        let conversation_dir = conversation_dir(agent_id, &handle.conversation_id)?;
        close_conversation_dir(agent_id, &handle.conversation_id, &conversation_dir, reason)?;
        Ok(Some(handle.conversation_id))
    }

    pub fn discard_agent(&self, agent_id: &str) -> io::Result<Option<String>> {
        let mut active = lock_active(&self.active)?;
        Ok(active.remove(agent_id).map(|handle| handle.conversation_id))
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
        at: event
            .created_at
            .clone()
            .unwrap_or_else(current_rfc3339_millis),
        kind,
        role,
        speaker_type: event.role.as_ref().map(speaker_type_from_role),
        text: event.text.clone(),
        tool: event.title.clone().or_else(|| event.command.clone()),
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
            .or_else(|| event.turn_id.clone()),
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

pub fn narrative_from_delivered_input(
    at: &str,
    text: &str,
    sender_agent_id: Option<&str>,
    seq: u64,
) -> ConversationNarrativeRecord {
    ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
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

fn conversation_dir(agent_id: &str, conversation_id: &str) -> io::Result<std::path::PathBuf> {
    agent_conversation_dir(agent_id, conversation_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe conversation path"))
}

fn index_path(agent_id: &str) -> io::Result<std::path::PathBuf> {
    agent_conversations_dir(agent_id)
        .map(|dir| dir.join("index.jsonl"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe agent path"))
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
    format!("conv_{timestamp}_{}", safe_agent_suffix(agent_id))
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
        .or_else(|| event.turn_id.clone())
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
    active: &mut HashMap<String, ActiveConversationHandle>,
    context: &ConversationArchiveContext,
    provider_source_key: Option<String>,
) -> io::Result<ActiveConversationHandle> {
    if let Some(existing) = active.get(&context.agent_id).cloned() {
        if existing.provider_source_key == provider_source_key {
            return Ok(existing);
        }
        close_conversation_handle(
            &context.agent_id,
            &existing,
            ConversationBoundaryReason::ProviderSourceChanged,
        )?;
        active.remove(&context.agent_id);
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
        if manifest.provider != context.provider {
            continue;
        }
        if manifest.provider_source_key.as_deref() != provider_source_key {
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
            provider_source_key: manifest.provider_source_key,
        }));
    }
    Ok(None)
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
        effective_logging: ConversationLoggingSetting::Enabled,
        created_at,
        updated_at,
        closed_at: None,
        status: ConversationStatus::Open,
        boundary_reason: ConversationBoundaryReason::Spawn,
        format_versions: ConversationFormatVersions::default(),
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
        artifact_count,
        path: format!(
            "agents/{}/conversations/{}",
            manifest.agent_id, manifest.conversation_id
        ),
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
        effective_conversation_logging, lifecycle_record, narrative_from_chat_event,
        narrative_from_delivered_input, ActiveConversationHandle, ConversationArchiveContext,
        ConversationArchiveState,
    };
    use wardian_core::conversations::{
        read_jsonl_records, AgentConversationLoggingSetting, ConversationBoundaryReason,
        ConversationLoggingSetting, ConversationManifest, ConversationNarrativeRecord,
        ConversationRecordKind, ConversationSourceRecord, ConversationSpeakerType,
        ConversationStatus, CONVERSATION_SCHEMA,
    };
    use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};
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
