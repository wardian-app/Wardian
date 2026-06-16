use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Mutex,
};

use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, read_jsonl_records, write_json_atomic,
    AgentConversationLoggingSetting, ConversationBoundaryReason, ConversationFormatVersions,
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationRecordKind, ConversationSpeakerType,
    ConversationStatus, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};
use wardian_core::paths::{agent_conversation_dir, agent_conversations_dir};

#[derive(Debug, Default)]
pub struct ConversationArchiveState {
    #[allow(dead_code)]
    active: Mutex<HashMap<String, ActiveConversationHandle>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConversationHandle {
    pub conversation_id: String,
    pub next_seq: u64,
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
    pub fn append_chat_events(
        &self,
        agent_id: &str,
        events: &[AgentChatEvent],
    ) -> io::Result<usize> {
        if !events
            .iter()
            .any(|event| record_kind_from_chat_event_kind(&event.kind).is_some())
        {
            return Ok(0);
        }

        let mut active = lock_active(&self.active)?;
        let mut handle =
            active
                .get(agent_id)
                .cloned()
                .unwrap_or_else(|| ActiveConversationHandle {
                    conversation_id: new_conversation_id(agent_id),
                    next_seq: 1,
                });
        let conversation_dir = conversation_dir(agent_id, &handle.conversation_id)?;
        let conversation_path = conversation_dir.join("conversation.jsonl");
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

        for event in events {
            if !seen_event_ids.insert(event.id.clone()) {
                continue;
            }
            let Some(record) = narrative_from_chat_event(event, next_seq) else {
                continue;
            };
            next_seq = next_seq.saturating_add(1);
            appended.push(record);
        }

        if appended.is_empty() {
            handle.next_seq = next_seq;
            active.insert(agent_id.to_string(), handle);
            return Ok(0);
        }

        for record in &appended {
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
        let provider = provider_from_events(events).unwrap_or_else(|| "unknown".to_string());
        let manifest = open_manifest(
            agent_id,
            &handle.conversation_id,
            &provider,
            first_record.at.clone(),
            last_record.at.clone(),
        );
        write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
        append_index_upsert(
            &index_path(agent_id)?,
            &index_entry_from_manifest(
                &manifest,
                None,
                excerpt_from_record(first_record),
                excerpt_from_record(last_record),
                record_count,
            ),
        )?;

        handle.next_seq = next_seq;
        active.insert(agent_id.to_string(), handle);
        Ok(appended.len())
    }

    pub fn append_delivered_input(
        &self,
        agent_id: &str,
        text: &str,
        sender_agent_id: Option<&str>,
    ) -> io::Result<usize> {
        if text.trim().is_empty() {
            return Ok(0);
        }

        self.append_generated_record(agent_id, "unknown", |seq| {
            narrative_from_delivered_input(&current_rfc3339_millis(), text, sender_agent_id, seq)
        })
    }

    pub fn append_lifecycle_boundary(
        &self,
        agent_id: &str,
        reason: ConversationBoundaryReason,
    ) -> io::Result<usize> {
        self.append_generated_record(agent_id, "unknown", |seq| {
            lifecycle_record(seq, reason, &current_rfc3339_millis())
        })
    }

    fn append_generated_record(
        &self,
        agent_id: &str,
        provider: &str,
        make_record: impl FnOnce(u64) -> ConversationNarrativeRecord,
    ) -> io::Result<usize> {
        let mut active = lock_active(&self.active)?;
        let mut handle =
            active
                .get(agent_id)
                .cloned()
                .unwrap_or_else(|| ActiveConversationHandle {
                    conversation_id: new_conversation_id(agent_id),
                    next_seq: 1,
                });
        let conversation_dir = conversation_dir(agent_id, &handle.conversation_id)?;
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
        let record = make_record(next_seq);
        append_jsonl_record(&conversation_path, &record)?;

        let first_record = existing_records.first().unwrap_or(&record);
        let record_count = (existing_records.len() + 1) as u64;
        let manifest = open_manifest(
            agent_id,
            &handle.conversation_id,
            provider,
            first_record.at.clone(),
            record.at.clone(),
        );
        write_json_atomic(&conversation_dir.join("manifest.json"), &manifest)?;
        append_index_upsert(
            &index_path(agent_id)?,
            &index_entry_from_manifest(
                &manifest,
                None,
                excerpt_from_record(first_record),
                excerpt_from_record(&record),
                record_count,
            ),
        )?;

        handle.next_seq = next_seq.saturating_add(1);
        active.insert(agent_id.to_string(), handle);
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
        let conversation_path = conversation_dir.join("conversation.jsonl");
        let records: Vec<ConversationNarrativeRecord> = read_jsonl_records(&conversation_path)?;
        let now = current_rfc3339_millis();
        let mut manifest =
            read_manifest(&conversation_dir.join("manifest.json"))?.unwrap_or_else(|| {
                let created_at = records
                    .first()
                    .map(|record| record.at.clone())
                    .unwrap_or_else(|| now.clone());
                open_manifest(
                    agent_id,
                    &handle.conversation_id,
                    "unknown",
                    created_at,
                    now.clone(),
                )
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
            ),
        )?;
        Ok(Some(handle.conversation_id))
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
        status: event.status.as_ref().map(|status| status_to_string(status)),
        summary: event.title.clone(),
        excerpt: None,
        event_refs: vec![event.id.clone()],
        source_refs: event.source.iter().cloned().collect(),
        artifact_refs: Vec::new(),
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
        AgentChatEventKind::Status => Some(ConversationRecordKind::Status),
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
    agent_id: &str,
    conversation_id: &str,
    provider: &str,
    created_at: String,
    updated_at: String,
) -> ConversationManifest {
    ConversationManifest {
        schema: CONVERSATION_SCHEMA,
        conversation_id: conversation_id.to_string(),
        agent_id: agent_id.to_string(),
        agent_name: agent_id.to_string(),
        agent_class: String::new(),
        workspace: String::new(),
        provider: provider.to_string(),
        provider_session_ids: Vec::new(),
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
        artifact_count: 0,
        path: format!(
            "agents/{}/conversations/{}",
            manifest.agent_id, manifest.conversation_id
        ),
    }
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
        narrative_from_delivered_input, ActiveConversationHandle, ConversationArchiveState,
    };
    use wardian_core::conversations::{
        read_jsonl_records, AgentConversationLoggingSetting, ConversationBoundaryReason,
        ConversationLoggingSetting, ConversationNarrativeRecord, ConversationRecordKind,
        ConversationSpeakerType, CONVERSATION_SCHEMA,
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
            },
        );

        let conversation_id = archive
            .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
            .expect("rollover succeeds");

        assert_eq!(conversation_id.as_deref(), Some("conv_existing_agent_1"));
        assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
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
            .append_chat_events("agent-1", &[event.clone()])
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

    fn isolated_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp.path());
        (guard, temp)
    }
}
