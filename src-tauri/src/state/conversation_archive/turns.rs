use std::collections::{BTreeMap, HashMap, HashSet};

use wardian_core::conversations::{
    ConversationManifest, ConversationNarrativeRecord, ConversationProviderNativeRef,
    ConversationRecordKind, ConversationSourceRecord, ConversationTurnFailureSignal,
    ConversationTurnRecord, ConversationTurnStatus, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatStatus};

#[derive(Debug)]
pub(super) struct ConversationArchiveSummary {
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

pub(super) fn derive_turn_records(
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

pub(super) fn archive_summary(
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

pub(super) fn apply_archive_summary_to_manifest(
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
