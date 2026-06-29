use std::collections::{BTreeMap, HashMap, HashSet};

use wardian_core::conversations::{
    ConversationManifest, ConversationNarrativeRecord, ConversationProviderNativeRef,
    ConversationRecordKind, ConversationSourceRecord, ConversationTurnAssistantResult,
    ConversationTurnCounts, ConversationTurnFailureSignal, ConversationTurnFiles,
    ConversationTurnRecord, ConversationTurnRecordRefs, ConversationTurnRequest,
    ConversationTurnSideEffect, ConversationTurnStatus, CONVERSATION_TURNS_SCHEMA,
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

#[cfg(test)]
pub(super) fn derive_turn_records(
    conversation_id: &str,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
    sources: &[ConversationSourceRecord],
    is_open: bool,
) -> Vec<ConversationTurnRecord> {
    derive_turn_records_with_context(
        conversation_id,
        records,
        events,
        sources,
        is_open,
        None,
        &[],
    )
}

pub(super) fn derive_turn_records_with_context(
    conversation_id: &str,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
    sources: &[ConversationSourceRecord],
    is_open: bool,
    fallback_provider: Option<&str>,
    fallback_provider_session_ids: &[String],
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
            turn_record_from_accumulator(
                conversation_id,
                (index + 1) as u64,
                turn,
                is_open_tail,
                fallback_provider,
                fallback_provider_session_ids,
            )
        })
        .collect()
}

fn turn_record_from_accumulator(
    conversation_id: &str,
    turn_index: u64,
    turn: TurnAccumulator,
    is_open: bool,
    fallback_provider: Option<&str>,
    fallback_provider_session_ids: &[String],
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
        let provider_file_paths = provider_file_paths_from_event(event);
        if !provider_file_paths.is_empty() {
            let tool_name = event_tool_name(event).unwrap_or_else(|| "unknown_tool".to_string());
            if provider_tool_reads_file(&tool_name) {
                extend_unique(&mut files_read, provider_file_paths.clone());
            }
            if provider_tool_writes_file(&tool_name) {
                extend_unique(&mut files_written, provider_file_paths.clone());
                extend_side_effects(
                    &mut external_side_effects,
                    vec![ConversationTurnSideEffect {
                        kind: "file_edit".to_string(),
                        evidence_seq: seq,
                        summary: side_effect_summary_with_paths(&tool_name, &provider_file_paths),
                        paths: provider_file_paths.clone(),
                    }],
                );
            }
            extend_unique(&mut files_mentioned, provider_file_paths);
        }
        let file_edit_paths = file_edit_paths_from_event(event);
        if !file_edit_paths.is_empty() {
            extend_unique(&mut files_written, file_edit_paths.clone());
            extend_unique(&mut files_mentioned, file_edit_paths.clone());
            extend_side_effects(
                &mut external_side_effects,
                vec![ConversationTurnSideEffect {
                    kind: "file_edit".to_string(),
                    evidence_seq: seq,
                    summary: side_effect_summary_with_paths("apply_patch", &file_edit_paths),
                    paths: file_edit_paths,
                }],
            );
        }
        if let Some(path) = event.path.as_deref().and_then(normalize_path) {
            extend_unique(&mut files_mentioned, vec![path]);
        }
        if let Some(command) = event.command.as_deref() {
            let command_paths = extract_mentioned_paths(command);
            if is_read_command(command) {
                extend_unique(&mut files_read, command_paths.clone());
            }
            extend_unique(&mut files_mentioned, command_paths);
        }
        if event.kind == AgentChatEventKind::Message {
            if let Some(text) = event.text.as_deref() {
                extend_unique(&mut files_mentioned, extract_mentioned_paths(text));
            }
        }
        extend_side_effects(
            &mut external_side_effects,
            metadata_string_array(&event.metadata, "external_side_effects")
                .into_iter()
                .map(|summary| ConversationTurnSideEffect {
                    kind: "external_side_effect".to_string(),
                    evidence_seq: seq,
                    summary,
                    paths: Vec::new(),
                })
                .collect(),
        );
        extend_side_effects(
            &mut external_side_effects,
            side_effects_from_event(seq, event),
        );
    }

    for record in &turn.records {
        let record_text = record.text.as_deref().or(record.excerpt.as_deref());
        if record.kind == ConversationRecordKind::Message {
            if let Some(text) = record_text {
                extend_unique(&mut files_mentioned, extract_mentioned_paths(text));
            }
        }
        if let Some(signal) = reported_verification_failure_from_record(record) {
            failure_signals.push(signal);
        }
        if record.kind == ConversationRecordKind::ToolCall
            && record.tool.as_deref() == Some("apply_patch")
        {
            let paths = record_text.map(extract_patch_paths).unwrap_or_default();
            if !paths.is_empty() {
                extend_unique(&mut files_written, paths.clone());
                extend_unique(&mut files_mentioned, paths.clone());
            }
            if paths.is_empty() && has_file_edit_side_effect(&external_side_effects) {
                continue;
            }
            extend_side_effects(
                &mut external_side_effects,
                vec![ConversationTurnSideEffect {
                    kind: "file_edit".to_string(),
                    evidence_seq: record.seq,
                    summary: side_effect_summary_with_paths("apply_patch", &paths),
                    paths,
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
    let mut request = request_record
        .map(request_from_record)
        .unwrap_or(ConversationTurnRequest {
            seq: seq_start,
            kind: "unknown".to_string(),
            text: None,
            text_truncated: false,
            objective_text: None,
            objective_text_truncated: None,
        });
    if request.kind == "unknown" && is_tool_only_turn(&turn.records) {
        request.kind = "tool_only".to_string();
        if request.text.is_none() {
            request.text = turn.records.first().and_then(tool_only_request_text);
        }
    }
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
    } else if request.kind == "agent_context" {
        (
            ConversationTurnStatus::ContextOnly,
            "mechanical_context_only".to_string(),
        )
    } else if request.kind == "tool_only" {
        (
            ConversationTurnStatus::ContextOnly,
            "mechanical_tool_only".to_string(),
        )
    } else if assistant_message.is_some() {
        (
            ConversationTurnStatus::Responded,
            "mechanical_assistant_message".to_string(),
        )
    } else if is_final_open_turn {
        (
            ConversationTurnStatus::PendingResponse,
            "mechanical_open_tail_pending_response".to_string(),
        )
    } else {
        (
            ConversationTurnStatus::PendingResponse,
            "mechanical_no_assistant_message".to_string(),
        )
    };
    let assistant_result = assistant_message.map(assistant_result_from_record);
    let event_refs = turn
        .records
        .iter()
        .flat_map(|record| record.event_refs.iter().cloned())
        .collect();

    ConversationTurnRecord {
        schema: CONVERSATION_TURNS_SCHEMA,
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
        provider_native_refs: provider_native_refs_from_sources(
            &turn.sources,
            fallback_provider,
            fallback_provider_session_ids,
        ),
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
        provider_native_refs: provider_native_refs_from_sources(sources, None, &[]),
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
    fallback_provider: Option<&str>,
    fallback_provider_session_ids: &[String],
) -> Vec<ConversationProviderNativeRef> {
    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    let fallback_provider = fallback_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    for source in sources {
        if source.provider == "wardian"
            || (source.provider_session_id.is_none() && source.source_path.is_none())
        {
            continue;
        }
        let provider_session_id = source.provider_session_id.clone().or_else(|| {
            (Some(source.provider.as_str()) == fallback_provider
                && fallback_provider_session_ids.len() == 1)
                .then(|| fallback_provider_session_ids[0].clone())
        });
        let key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            source.provider,
            provider_session_id.as_deref().unwrap_or_default(),
            source.source_kind,
            source.source_path.as_deref().unwrap_or_default()
        );
        if seen.insert(key) {
            refs.push(ConversationProviderNativeRef {
                provider: source.provider.clone(),
                provider_session_id,
                source_kind: source.source_kind.clone(),
                source_path: source.source_path.clone(),
            });
        }
    }
    if refs.is_empty() {
        if let Some(provider) = fallback_provider {
            for session_id in fallback_provider_session_ids {
                let session_id = session_id.trim();
                if session_id.is_empty() {
                    continue;
                }
                let key = format!("{provider}\u{1f}{session_id}\u{1f}provider_session\u{1f}");
                if seen.insert(key) {
                    refs.push(ConversationProviderNativeRef {
                        provider: provider.to_string(),
                        provider_session_id: Some(session_id.to_string()),
                        source_kind: "provider_session".to_string(),
                        source_path: None,
                    });
                }
            }
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

fn is_tool_only_turn(records: &[ConversationNarrativeRecord]) -> bool {
    !records.is_empty()
        && records.iter().all(|record| {
            matches!(
                record.kind,
                ConversationRecordKind::ToolCall
                    | ConversationRecordKind::ToolResult
                    | ConversationRecordKind::Approval
                    | ConversationRecordKind::Error
                    | ConversationRecordKind::Status
            )
        })
}

fn tool_only_request_text(record: &ConversationNarrativeRecord) -> Option<String> {
    record
        .summary
        .as_deref()
        .or(record.tool.as_deref())
        .or(record.status.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn request_from_record(record: &ConversationNarrativeRecord) -> ConversationTurnRequest {
    let (text, text_truncated) = bounded_record_text(record);
    let kind = request_kind_for_record(record);
    let (objective_text, objective_text_truncated) = objective_text_for_request(&kind, record);
    ConversationTurnRequest {
        seq: record.seq,
        objective_text,
        objective_text_truncated,
        kind,
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
        paths: Vec::new(),
    }
}

fn reported_verification_failure_from_record(
    record: &ConversationNarrativeRecord,
) -> Option<ConversationTurnFailureSignal> {
    if record.kind != ConversationRecordKind::Message || record.role.as_deref() != Some("assistant")
    {
        return None;
    }
    let text = record.text.as_deref().or(record.excerpt.as_deref())?;
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("fail") && lower.contains("verification")) {
        return None;
    }
    let command = reported_failed_command(text).or_else(|| {
        [
            "npm run check",
            "npm test",
            "npm run build",
            "cargo test",
            "cargo check",
            "cargo clippy",
        ]
        .iter()
        .find(|command| lower.contains(**command))
        .map(|command| (*command).to_string())
    });
    let command = command?;
    if !reports_unresolved_verification_failure(&lower) {
        return None;
    }
    Some(ConversationTurnFailureSignal {
        kind: "reported_verification_failure".to_string(),
        seq: record.seq,
        tool: Some(command),
        summary: compact_text(text, 240),
    })
}

fn reports_unresolved_verification_failure(lower_text: &str) -> bool {
    [
        "still fails",
        "still failing",
        "failed verification",
        "verification failed",
        "verification failure",
        "check failed",
        "test failed",
        "tests failed",
        "build failed",
        "clippy failed",
    ]
    .iter()
    .any(|phrase| lower_text.contains(phrase))
}

fn reported_failed_command(text: &str) -> Option<String> {
    let parts = text.split('`').collect::<Vec<_>>();
    for index in (1..parts.len()).step_by(2) {
        let command = parts[index];
        let before = parts
            .get(index.saturating_sub(1))
            .map(|value| {
                value
                    .chars()
                    .rev()
                    .take(80)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            })
            .unwrap_or_default();
        let after = parts
            .get(index + 1)
            .map(|value| value.chars().take(160).collect::<String>())
            .unwrap_or_default();
        let nearby = format!("{before}{after}").to_ascii_lowercase();
        if nearby.contains("fail") && looks_like_verification_command(command) {
            return Some(command.trim().to_string());
        }
    }
    None
}

fn looks_like_verification_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    ["npm ", "npm run ", "cargo ", "pnpm ", "yarn ", "node "]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn file_edit_paths_from_event(event: &AgentChatEvent) -> Vec<String> {
    let mut paths = Vec::new();
    if is_apply_patch_event(event) {
        if let Some(text) = event.text.as_deref() {
            extend_unique(&mut paths, extract_patch_paths(text));
        }
        if let Some(input_text) = metadata_string(&event.metadata, "tool_input_text") {
            extend_unique(&mut paths, extract_patch_paths(&input_text));
        }
    }
    if let Some(text) = event.text.as_deref() {
        extend_unique(&mut paths, extract_apply_patch_result_paths(text));
    }
    paths
}

fn provider_file_paths_from_event(event: &AgentChatEvent) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = event.path.as_deref().and_then(normalize_provider_path) {
        extend_unique(&mut paths, vec![path]);
    }
    if let Some(path) = metadata_string(&event.metadata, "file_path")
        .and_then(|path| normalize_provider_path(&path))
    {
        extend_unique(&mut paths, vec![path]);
    }
    for key in [
        "file_path",
        "AbsolutePath",
        "TargetFile",
        "FilePath",
        "path",
        "uri",
        "fileUri",
    ] {
        if let Some(path) = event
            .metadata
            .get("tool_input")
            .and_then(|input| input.get(key))
            .and_then(|value| value.as_str())
            .and_then(normalize_provider_path)
        {
            extend_unique(&mut paths, vec![path]);
        }
    }
    extend_unique(
        &mut paths,
        metadata_string_array(&event.metadata, "files_read")
            .into_iter()
            .filter_map(|path| normalize_provider_path(&path))
            .collect(),
    );
    extend_unique(
        &mut paths,
        metadata_string_array(&event.metadata, "files_written")
            .into_iter()
            .filter_map(|path| normalize_provider_path(&path))
            .collect(),
    );
    if let Some(text) = event.text.as_deref() {
        extend_unique(&mut paths, extract_file_uri_paths(text));
    }
    paths
}

fn provider_tool_reads_file(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "read" | "view file" | "view_file"
    )
}

fn provider_tool_writes_file(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "edit"
            | "write"
            | "multiedit"
            | "notebookedit"
            | "write file"
            | "edit file"
            | "code action"
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
    )
}

fn is_apply_patch_event(event: &AgentChatEvent) -> bool {
    event_tool_name(event).as_deref() == Some("apply_patch")
}

fn first_github_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ')' | ']' | ',')))
        .find(|token| token.starts_with("https://github.com/"))
        .map(ToString::to_string)
}

fn objective_text_for_request(
    kind: &str,
    record: &ConversationNarrativeRecord,
) -> (Option<String>, Option<bool>) {
    let Some(text) = record.text.as_deref().or(record.excerpt.as_deref()) else {
        return (None, None);
    };
    let objective = match kind {
        "goal_start" => text.trim_start_matches("/goal").trim(),
        "goal_continuation" => goal_continuation_objective(text),
        _ => "",
    };
    let (text, truncated) = compact_text_with_truncation(objective, 500);
    (text, truncated.then_some(true))
}

fn goal_continuation_objective(text: &str) -> &str {
    let inner = text
        .split_once("<codex_internal_context source=\"goal\">")
        .map(|(_, rest)| rest)
        .unwrap_or(text)
        .split_once("</codex_internal_context>")
        .map(|(before, _)| before)
        .unwrap_or(text);
    if let Some(objective) = tag_text(inner, "objective") {
        return objective;
    }
    inner
        .lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("Goal:")
                .or_else(|| line.strip_prefix("Objective:"))
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| {
            inner
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('<'))
                .unwrap_or(inner)
        })
}

fn tag_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim()).filter(|value| !value.is_empty())
}

fn compact_text(text: &str, limit_chars: usize) -> Option<String> {
    compact_text_with_truncation(text, limit_chars).0
}

fn compact_text_with_truncation(text: &str, limit_chars: usize) -> (Option<String>, bool) {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return (None, false);
    }
    let mut end = 0;
    for (index, ch) in compact.char_indices() {
        if index >= limit_chars {
            break;
        }
        end = index + ch.len_utf8();
    }
    if compact.chars().count() <= limit_chars {
        (Some(compact), false)
    } else {
        (Some(compact[..end].to_string()), true)
    }
}

fn extract_patch_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in text.lines().map(str::trim) {
        let path = line
            .strip_prefix("*** Update File:")
            .or_else(|| line.strip_prefix("*** Add File:"))
            .or_else(|| line.strip_prefix("*** Delete File:"))
            .or_else(|| line.strip_prefix("*** Move to:"))
            .map(str::trim)
            .and_then(normalize_path);
        if let Some(path) = path {
            extend_unique(&mut paths, vec![path]);
        }
    }
    paths
}

fn extract_apply_patch_result_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut in_updated_files = false;
    for line in text.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("success. updated the following files:") {
            in_updated_files = true;
            continue;
        }
        if !in_updated_files || line.is_empty() {
            continue;
        }
        let candidate = line
            .split_once(char::is_whitespace)
            .and_then(|(status, path)| {
                (status.len() == 1
                    && matches!(status.as_bytes()[0], b'A' | b'M' | b'D' | b'R' | b'C'))
                .then_some(path)
            })
            .unwrap_or(line)
            .trim();
        if let Some(path) = normalize_path(candidate) {
            extend_unique(&mut paths, vec![path]);
        }
    }
    paths
}

fn extract_mentioned_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in text.split_whitespace() {
        if let Some(path) = normalize_path(token) {
            extend_unique(&mut paths, vec![path]);
        }
    }
    paths
}

fn extract_file_uri_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in text.split_whitespace() {
        let Some(index) = token.find("file://") else {
            continue;
        };
        if let Some(path) = normalize_provider_path(&token[index..]) {
            extend_unique(&mut paths, vec![path]);
        }
    }
    paths
}

fn normalize_provider_path(value: &str) -> Option<String> {
    let mut trimmed = unquote_jsonish_string(value)
        .trim()
        .trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | ',' | ';' | ')' | '(' | ']' | '[' | '{' | '}'
            )
        })
        .trim_end_matches('.')
        .to_string();
    if trimmed.is_empty() || trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return None;
    }
    if trimmed.to_ascii_lowercase().starts_with("file://") {
        trimmed = trimmed["file://".len()..].to_string();
        if trimmed.starts_with('/') && trimmed.as_bytes().get(2) == Some(&b':') {
            trimmed.remove(0);
        }
        trimmed = percent_decode_path(&trimmed);
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '*' | '?' | '<' | '>' | '|' | '{' | '}'))
    {
        return None;
    }
    let mut without_line = trimmed.trim();
    while let Some((path, suffix)) = without_line.rsplit_once(':') {
        if suffix.chars().all(|ch| ch.is_ascii_digit()) && !is_windows_drive_path(without_line) {
            without_line = path;
        } else {
            break;
        }
    }
    let normalized = without_line.replace('\\', "/");
    let normalized = normalized.trim_matches('/').to_string();
    if normalized.is_empty()
        || normalized
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return None;
    }
    Some(repo_relative_path(&normalized))
}

fn unquote_jsonish_string(value: &str) -> String {
    let mut text = value.trim().to_string();
    for _ in 0..3 {
        let trimmed = text.trim();
        if !(trimmed.starts_with('"') && trimmed.ends_with('"')) {
            break;
        }
        let Ok(decoded) = serde_json::from_str::<String>(trimmed) else {
            break;
        };
        text = decoded;
    }
    text
}

fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hi = hex_value(bytes[index + 1]);
            let lo = hex_value(bytes[index + 2]);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                decoded.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn is_windows_drive_path(value: &str) -> bool {
    value.len() >= 3
        && value.as_bytes()[1] == b':'
        && value.as_bytes()[0].is_ascii_alphabetic()
        && matches!(value.as_bytes()[2], b'/' | b'\\')
}

fn normalize_path(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | ',' | ';' | ':' | ')' | '(' | ']' | '[' | '{' | '}'
            )
        })
        .trim_end_matches('.');
    if trimmed.is_empty() || trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return None;
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '*' | '?' | '<' | '>' | '|' | '{' | '}' | ','))
    {
        return None;
    }
    let mut without_line = trimmed;
    while let Some((path, suffix)) = without_line.rsplit_once(':') {
        if suffix.chars().all(|ch| ch.is_ascii_digit()) {
            without_line = path;
        } else {
            break;
        }
    }
    if without_line.contains(':') {
        return None;
    }
    let normalized = repo_relative_path(&without_line.replace('\\', "/"));
    if normalized
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return None;
    }
    let known_file = matches!(
        normalized.as_str(),
        "Cargo.toml" | "package.json" | "package-lock.json" | "README.md" | "AGENTS.md"
    );
    let known_prefix = [
        "src/",
        "src-tauri/",
        "crates/",
        "docs/",
        "e2e/",
        "e2e-native/",
        "scripts/",
        "tools/",
        ".github/",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix));
    let has_extension = normalized
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.') && !name.starts_with('.'));
    if known_file || (known_prefix && (has_extension || normalized.ends_with('/'))) {
        Some(normalized)
    } else {
        None
    }
}

fn repo_relative_path(path: &str) -> String {
    let known_prefixes = [
        "src-tauri/",
        "crates/",
        "docs/",
        "e2e-native/",
        "e2e/",
        "scripts/",
        "tools/",
        ".github/",
        "src/",
    ];
    if known_prefixes.iter().any(|prefix| path.starts_with(prefix))
        || matches!(
            path,
            "Cargo.toml" | "package.json" | "package-lock.json" | "README.md" | "AGENTS.md"
        )
    {
        return path.to_string();
    }
    for prefix in known_prefixes {
        if let Some(index) = path.find(prefix) {
            return path[index..].to_string();
        }
    }
    path.to_string()
}

fn is_read_command(command: &str) -> bool {
    let lower = command.trim_start().to_ascii_lowercase();
    [
        "cat ",
        "type ",
        "more ",
        "less ",
        "sed ",
        "rg ",
        "grep ",
        "head ",
        "tail ",
        "get-content ",
        "gc ",
        "select-string ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn side_effect_summary_with_paths(summary: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}: {}", paths.join(", "))
    }
}

fn extend_side_effects(
    values: &mut Vec<ConversationTurnSideEffect>,
    next_values: Vec<ConversationTurnSideEffect>,
) {
    let mut seen = values
        .iter()
        .map(side_effect_dedupe_key)
        .collect::<HashSet<_>>();
    for value in next_values {
        let key = side_effect_dedupe_key(&value);
        if seen.insert(key) {
            values.push(value);
        }
    }
}

fn side_effect_dedupe_key(value: &ConversationTurnSideEffect) -> String {
    if value.kind == "file_edit" {
        return format!("{}\u{1f}{}", value.kind, value.paths.join("\u{1e}"));
    }
    if matches!(
        value.kind.as_str(),
        "github_pr" | "github_issue" | "github_url"
    ) {
        return format!(
            "{}\u{1f}{}\u{1f}{}",
            value.kind,
            value.summary,
            value.paths.join("\u{1e}")
        );
    }
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        value.kind,
        value.evidence_seq,
        value.summary,
        value.paths.join("\u{1e}")
    )
}

fn has_file_edit_side_effect(values: &[ConversationTurnSideEffect]) -> bool {
    values.iter().any(|value| value.kind == "file_edit")
}
