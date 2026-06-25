use std::collections::{BTreeMap, HashMap, HashSet};

use wardian_core::conversations::{
    ConversationManifest, ConversationNarrativeRecord, ConversationProviderNativeRef,
    ConversationRecordKind, ConversationSourceRecord, ConversationTurnAssistantResult,
    ConversationTurnCounts, ConversationTurnFailureSignal, ConversationTurnFiles,
    ConversationTurnRecord, ConversationTurnRecordRefs, ConversationTurnRequest,
    ConversationTurnSideEffect, ConversationTurnStatus, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatStatus};

use super::records::metadata_string;

const TURN_TEXT_LIMIT_CHARS: usize = 4_000;

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
    records: Vec<ConversationNarrativeRecord>,
    events: Vec<AgentChatEvent>,
    sources: Vec<ConversationSourceRecord>,
}

pub(super) fn derive_turn_records(
    conversation_id: &str,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
    sources: &[ConversationSourceRecord],
    is_open: bool,
) -> Vec<ConversationTurnRecord> {
    let events_by_id = events
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect::<HashMap<_, _>>();
    let sources_by_id = sources
        .iter()
        .map(|source| (source.source_id.as_str(), source))
        .collect::<HashMap<_, _>>();
    let mut ordered_records = records.iter().collect::<Vec<_>>();
    ordered_records.sort_by_key(|record| record.seq);

    let mut turns: Vec<TurnAccumulator> = Vec::new();
    let mut current_turn: Option<usize> = None;

    for record in ordered_records {
        let starts_request_turn = is_user_request_record(record);
        let starts_lifecycle_turn =
            record.kind == ConversationRecordKind::Lifecycle && current_turn.is_none();
        let turn_index = match current_turn {
            Some(index) if !starts_request_turn && !starts_lifecycle_turn => index,
            _ => {
                turns.push(TurnAccumulator {
                    records: Vec::new(),
                    events: Vec::new(),
                    sources: Vec::new(),
                });
                let index = turns.len() - 1;
                current_turn = Some(index);
                index
            }
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

    let turn_count = turns.len();
    turns
        .into_iter()
        .enumerate()
        .map(|(index, turn)| {
            let is_open_tail = is_open && index + 1 == turn_count;
            turn_record_from_accumulator(conversation_id, (index + 1) as u64, turn, is_open_tail)
        })
        .collect()
}

fn turn_record_from_accumulator(
    conversation_id: &str,
    turn_index: u64,
    turn: TurnAccumulator,
    is_open: bool,
) -> ConversationTurnRecord {
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
    let started_at = turn
        .records
        .iter()
        .min_by_key(|record| record.seq)
        .map(|record| record.at.clone())
        .unwrap_or_default();
    let updated_at = turn
        .records
        .iter()
        .max_by_key(|record| record.seq)
        .map(|record| record.at.clone())
        .unwrap_or_else(|| started_at.clone());
    let request_record = turn
        .records
        .iter()
        .find(|record| is_user_request_record(record))
        .or_else(|| turn.records.first());
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

    let mut counts = ConversationTurnCounts {
        records: turn.records.len() as u64,
        assistant_messages: turn
            .records
            .iter()
            .filter(|record| {
                record.kind == ConversationRecordKind::Message
                    && record.role.as_deref() == Some("assistant")
            })
            .count() as u64,
        tool_calls: turn
            .records
            .iter()
            .filter(|record| record.kind == ConversationRecordKind::ToolCall)
            .count() as u64,
        tool_results: turn
            .records
            .iter()
            .filter(|record| record.kind == ConversationRecordKind::ToolResult)
            .count() as u64,
        nonzero_tool_results: 0,
        failed_tool_results: 0,
        timeouts: 0,
    };
    let mut failure_signals = Vec::new();
    let mut files_read = Vec::new();
    let mut files_written = Vec::new();
    let mut files_mentioned = Vec::new();
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
        if event.kind == AgentChatEventKind::ToolResult
            && event.status == Some(AgentChatStatus::Failed)
        {
            counts.failed_tool_results += 1;
            failure_signals.push(ConversationTurnFailureSignal {
                kind: "tool_failed".to_string(),
                seq,
                tool: event_tool_name(event),
                summary: Some("tool status failed".to_string()),
            });
        }
        if let Some(exit_code) = event.exit_code.filter(|exit_code| *exit_code != 0) {
            counts.nonzero_tool_results += 1;
            failure_signals.push(ConversationTurnFailureSignal {
                kind: "command_nonzero_exit".to_string(),
                seq,
                tool: event_tool_name(event),
                summary: Some(format!("Exit code: {exit_code}")),
            });
        }
        if is_timeout_event(event) {
            counts.timeouts += 1;
            failure_signals.push(ConversationTurnFailureSignal {
                kind: "tool_timeout".to_string(),
                seq,
                tool: event_tool_name(event),
                summary: Some("tool timeout".to_string()),
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
            &mut files_mentioned,
            metadata_string_array(&event.metadata, "files_mentioned"),
        );
        extend_side_effects(
            &mut external_side_effects,
            metadata_string_array(&event.metadata, "external_side_effects")
                .into_iter()
                .map(|summary| ConversationTurnSideEffect {
                    kind: "external_side_effect".to_string(),
                    evidence_seq: seq,
                    summary,
                })
                .collect(),
        );
        extend_side_effects(
            &mut external_side_effects,
            side_effects_from_event(seq, event),
        );
    }

    for record in &turn.records {
        if record.kind == ConversationRecordKind::ToolCall
            && record.tool.as_deref() == Some("apply_patch")
        {
            extend_side_effects(
                &mut external_side_effects,
                vec![ConversationTurnSideEffect {
                    kind: "file_edit".to_string(),
                    evidence_seq: record.seq,
                    summary: "apply_patch".to_string(),
                }],
            );
        }
        if record.kind == ConversationRecordKind::Error {
            failure_signals.push(ConversationTurnFailureSignal {
                kind: "tool_failed".to_string(),
                seq: record.seq,
                tool: record.tool.clone(),
                summary: record.summary.clone().or_else(|| record.status.clone()),
            });
        }
    }

    let lifecycle_only = turn
        .records
        .iter()
        .all(|record| record.kind == ConversationRecordKind::Lifecycle);
    let interrupted = turn.records.iter().any(is_interruption_record);
    let is_final_open_turn = is_open;
    let (status, status_source) = if interrupted {
        (
            ConversationTurnStatus::Interrupted,
            "mechanical_lifecycle_marker".to_string(),
        )
    } else if lifecycle_only {
        (
            ConversationTurnStatus::Lifecycle,
            "mechanical_lifecycle_only".to_string(),
        )
    } else if assistant_message.is_some() {
        (
            ConversationTurnStatus::Responded,
            "mechanical_assistant_message".to_string(),
        )
    } else if is_final_open_turn {
        (
            ConversationTurnStatus::InProgress,
            "mechanical_open_tail".to_string(),
        )
    } else {
        (
            ConversationTurnStatus::Unknown,
            "mechanical_grouping".to_string(),
        )
    };

    let request = request_record
        .map(request_from_record)
        .unwrap_or(ConversationTurnRequest {
            seq: seq_start,
            kind: "unknown".to_string(),
            text: None,
            text_truncated: false,
        });
    let assistant_result = assistant_message.map(assistant_result_from_record);
    let event_refs = turn
        .records
        .iter()
        .flat_map(|record| record.event_refs.iter().cloned())
        .collect();

    ConversationTurnRecord {
        schema: CONVERSATION_SCHEMA,
        conversation_id: conversation_id.to_string(),
        turn_index,
        turn_key: format!("{conversation_id}:turn:{turn_index:06}"),
        status,
        status_source,
        seq_start,
        seq_end,
        started_at,
        updated_at,
        request,
        assistant_result,
        counts,
        tools_used,
        files: ConversationTurnFiles {
            read: files_read,
            written: files_written,
            mentioned: files_mentioned,
        },
        external_side_effects,
        failure_signals,
        record_refs: ConversationTurnRecordRefs {
            conversation_seq_start: seq_start,
            conversation_seq_end: seq_end,
            event_refs,
        },
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

fn is_user_request_record(record: &ConversationNarrativeRecord) -> bool {
    record.kind == ConversationRecordKind::Message && record.role.as_deref() == Some("user")
}

fn request_from_record(record: &ConversationNarrativeRecord) -> ConversationTurnRequest {
    let (text, text_truncated) = bounded_record_text(record);
    ConversationTurnRequest {
        seq: record.seq,
        kind: request_kind_for_record(record),
        text,
        text_truncated,
    }
}

fn assistant_result_from_record(
    record: &ConversationNarrativeRecord,
) -> ConversationTurnAssistantResult {
    let (text, text_truncated) = bounded_record_text(record);
    ConversationTurnAssistantResult {
        seq: record.seq,
        text,
        text_truncated,
    }
}

fn request_kind_for_record(record: &ConversationNarrativeRecord) -> String {
    if record.kind == ConversationRecordKind::Lifecycle {
        return "lifecycle".to_string();
    }
    if record.kind != ConversationRecordKind::Message || record.role.as_deref() != Some("user") {
        return "unknown".to_string();
    }
    let text = record
        .text
        .as_deref()
        .or(record.excerpt.as_deref())
        .unwrap_or_default()
        .trim();
    if text.starts_with("/goal") {
        "goal_start".to_string()
    } else if text.contains("<codex_internal_context source=\"goal\">") {
        "goal_continuation".to_string()
    } else if text.starts_with("# AGENTS.md instructions")
        || text.contains("AGENTS.md instructions for")
    {
        "agent_context".to_string()
    } else if text.is_empty() {
        "unknown_user_message".to_string()
    } else {
        "user_request".to_string()
    }
}

fn bounded_record_text(record: &ConversationNarrativeRecord) -> (Option<String>, bool) {
    let source_text = record.text.as_deref().or(record.excerpt.as_deref());
    let Some(text) = source_text else {
        return (None, false);
    };
    let mut truncated = record.text.is_none() && record.excerpt.is_some();
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        if index >= TURN_TEXT_LIMIT_CHARS {
            truncated = true;
            break;
        }
        end = index + ch.len_utf8();
    }
    if text.chars().count() <= TURN_TEXT_LIMIT_CHARS {
        (Some(text.to_string()), truncated)
    } else {
        (Some(text[..end].to_string()), true)
    }
}

fn is_interruption_record(record: &ConversationNarrativeRecord) -> bool {
    if record.kind != ConversationRecordKind::Lifecycle {
        return false;
    }
    matches!(
        record.status.as_deref(),
        Some("shutdown" | "provider_source_changed")
    )
}

fn is_timeout_event(event: &AgentChatEvent) -> bool {
    event
        .metadata
        .get("timeout")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        || event
            .metadata
            .get("timed_out")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
}

fn event_tool_name(event: &AgentChatEvent) -> Option<String> {
    metadata_string(&event.metadata, "tool_name")
        .or_else(|| event.title.clone())
        .map(|tool| tool.trim().to_string())
        .filter(|tool| !tool.is_empty())
}

fn side_effects_from_event(seq: u64, event: &AgentChatEvent) -> Vec<ConversationTurnSideEffect> {
    let mut effects = Vec::new();
    let command = event.command.as_deref().unwrap_or_default();
    let text = event.text.as_deref().unwrap_or_default();
    let lower_command = command.to_ascii_lowercase();
    if lower_command.contains("git commit") {
        effects.push(side_effect(seq, "git_commit", "commit created"));
    }
    if lower_command.contains("git push") {
        effects.push(side_effect(seq, "git_push", "git push"));
    }
    if lower_command.contains("wardian agent spawn") {
        effects.push(side_effect(
            seq,
            "wardian_agent_spawn",
            "wardian agent spawn",
        ));
    }
    if lower_command.contains("wardian agent kill") {
        effects.push(side_effect(seq, "wardian_agent_kill", "wardian agent kill"));
    }
    if lower_command.contains("wardian send") {
        effects.push(side_effect(seq, "wardian_send", "wardian send"));
    }
    if lower_command.contains("wardian reply") {
        effects.push(side_effect(seq, "wardian_reply", "wardian reply"));
    }
    if lower_command.contains("npm install")
        || lower_command.contains("pnpm add")
        || lower_command.contains("yarn add")
        || lower_command.contains("cargo add")
    {
        effects.push(side_effect(seq, "package_install", "package install"));
    }
    if let Some(url) = first_github_url(text) {
        let kind = if url.contains("/pull/") {
            "github_pr"
        } else if url.contains("/issues/") {
            "github_issue"
        } else {
            "github_url"
        };
        effects.push(side_effect(seq, kind, &url));
    }
    effects
}

fn side_effect(seq: u64, kind: &str, summary: &str) -> ConversationTurnSideEffect {
    ConversationTurnSideEffect {
        kind: kind.to_string(),
        evidence_seq: seq,
        summary: summary.to_string(),
    }
}

fn first_github_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ')' | ']' | ',')))
        .find(|token| token.starts_with("https://github.com/"))
        .map(ToString::to_string)
}

fn extend_side_effects(
    values: &mut Vec<ConversationTurnSideEffect>,
    next_values: Vec<ConversationTurnSideEffect>,
) {
    let mut seen = values
        .iter()
        .map(|value| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                value.kind, value.evidence_seq, value.summary
            )
        })
        .collect::<HashSet<_>>();
    for value in next_values {
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            value.kind, value.evidence_seq, value.summary
        );
        if seen.insert(key) {
            values.push(value);
        }
    }
}
