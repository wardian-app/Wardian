//! Source-bound enrichment shared by capture and live archive replay.
//!
//! IDs and verified aliases identify observations. Text, timestamps and tail
//! sequence numbers are deliberately not identity evidence.
use std::collections::{BTreeSet, HashSet};
use std::io;

use serde_json::Value;
use wardian_core::conversations::ConversationNarrativeRecord;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

use super::{event_identity_ids, narrative_from_chat_event};

const PROVENANCE_KEYS: &[&str] = &[
    "input_origin",
    "input_purpose",
    "request_root_id",
    "causal_ref",
    "context_observation",
    "provider_turn_id",
    "codex_user_text_sha256",
    "provider_step_source",
];

fn string<'a>(event: &'a AgentChatEvent, key: &str) -> Option<&'a str> {
    event
        .metadata
        .get(key)?
        .as_str()
        .filter(|s| !s.trim().is_empty())
}

/// Require Wardian identity, provider and a common native source binding even
/// for an equal event ID. A conflicting explicit native session fails closed.
pub(crate) fn same_observation(old: &AgentChatEvent, current: &AgentChatEvent) -> bool {
    if old.session_id != current.session_id
        || old.provider != current.provider
        || old.kind != current.kind
        || current.metadata["provider_log"] != true
    {
        return false;
    }
    let old_session =
        string(old, "provider_session_id").or_else(|| string(old, "opencode_session_id"));
    let new_session =
        string(current, "provider_session_id").or_else(|| string(current, "opencode_session_id"));
    if matches!((old_session, new_session), (Some(a), Some(b)) if a != b) {
        return false;
    }
    let paths_match = string(old, "log_path")
        .zip(string(current, "log_path"))
        .is_some_and(|(a, b)| a == b);
    let sessions_match = old_session.zip(new_session).is_some_and(|(a, b)| a == b);
    if !paths_match && !sessions_match {
        return false;
    }
    // A framed native observation owns its physical source coordinate. Legacy
    // aliases, repeated text and a shared turn cannot transfer that ownership.
    // In particular, historical enrichment of G is not evidence linking G to N.
    match (
        string(old, "chat_source_ref"),
        string(current, "chat_source_ref"),
    ) {
        (None, None) => {}
        (Some(a), Some(b)) if a == b => {
            if old.metadata["generated"] == true || current.metadata["generated"] == true {
                return false;
            }
            if matches!(
                (string(old, "chat_source_epoch"), string(current, "chat_source_epoch")),
                (Some(a), Some(b)) if a != b
            ) {
                return false;
            }
        }
        _ => return false,
    }
    let current_ids = event_identity_ids(current);
    let shares_identity = event_identity_ids(old)
        .iter()
        .any(|id| !id.is_empty() && current_ids.contains(id));
    if !shares_identity {
        return false;
    }
    if old.id != current.id {
        if is_codex_user_message_mirror(old) && is_codex_user_message_mirror(current) {
            return match (
                string(old, "provider_turn_id"),
                string(current, "provider_turn_id"),
            ) {
                (Some(old_turn), Some(current_turn)) => old_turn == current_turn,
                _ => old.sequence.is_some() && old.sequence == current.sequence,
            };
        }
        if is_codex_user_message_mirror(old) || is_codex_user_message_mirror(current) {
            return crate::providers::chat_transcript::codex_user_mirror_pair(old, current);
        }
    }
    true
}

pub(super) fn is_codex_user_message_mirror(event: &AgentChatEvent) -> bool {
    event.provider == "codex"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && event.source.as_deref() == Some("event_msg")
        && string(event, "raw_type") == Some("user_message")
        && string(event, "input_origin") == Some("human_input")
}

pub(crate) fn canonicalize_role(event: &mut AgentChatEvent) {
    if event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && matches!(
            string(event, "input_origin"),
            Some("context_injection" | "provider_internal")
        )
    {
        event.role = Some(AgentChatRole::System);
    } else if event.kind == AgentChatEventKind::ToolResult
        && event.role == Some(AgentChatRole::User)
    {
        event.role = Some(AgentChatRole::Tool);
    }
}

/// Antigravity's observed SQLite GENERIC tool result has mutable output.
/// Status 3 means DONE, not success; no other provider/status layout is inferred.
pub(super) fn completed_native_tool(event: &AgentChatEvent) -> bool {
    event.provider == "antigravity"
        && event.kind == AgentChatEventKind::ToolResult
        && event.metadata["provider_log"] == true
        && event.metadata["log_source"] == "antigravity_conversation_database"
        && event.metadata["provider_step_type"] == 132
        && event.metadata["provider_step_source"] == 2
        && event.metadata["provider_step_status"] == 3
        && event.metadata["step_index"].as_u64().is_some()
        && event.metadata["tool_ordinal"].as_u64().is_some()
}

fn refresh_tool_completion(old: &mut AgentChatEvent, current: &AgentChatEvent) {
    // Called only after identity/source binding. Require the complete observed
    // location on both sides; missing native evidence cannot authorize a rewrite.
    if !completed_native_tool(current)
        || old.metadata["provider_step_status"] != 2
        || ![
            "log_source",
            "provider_step_type",
            "provider_step_source",
            "step_index",
            "tool_ordinal",
        ]
        .iter()
        .all(|key| old.metadata[*key] == current.metadata[*key])
        || current
            .text
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
    {
        return;
    }
    old.text = current.text.clone();
    old.metadata["provider_step_status"] = current.metadata["provider_step_status"].clone();
    // The newly observed text supersedes any materialized running placeholder.
    if let Some(metadata) = old.metadata.as_object_mut() {
        metadata.remove("text_excerpt");
        metadata.remove("text_artifact_refs");
    }
}

fn enrich(old: &mut AgentChatEvent, current: &AgentChatEvent) -> io::Result<()> {
    if old.kind == AgentChatEventKind::ToolResult {
        for key in [
            "step_index",
            "tool_ordinal",
            "provider_step_type",
            "log_source",
        ] {
            if let (Some(a), Some(b)) = (old.metadata.get(key), current.metadata.get(key)) {
                if !a.is_null() && !b.is_null() && a != b {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("conflicting native tool location: {key}"),
                    ));
                }
            }
        }
    }
    // An adapter downgrade must not replace native source evidence with its
    // older role-based fallback. Missing fields never erase known evidence.
    let weaker = old.metadata.get("provider_step_source").is_some()
        && current.metadata.get("provider_step_source").is_none();
    if let (Some(a), Some(b)) = (
        old.metadata.get("provider_step_source"),
        current.metadata.get("provider_step_source"),
    ) {
        if !a.is_null() && !b.is_null() && a != b {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "conflicting native archive step source",
            ));
        }
    }
    refresh_tool_completion(old, current);
    let broker_input = old.metadata["generated"] == true;
    if !weaker && !broker_input {
        for key in ["request_root_id", "provider_step_source"] {
            if let (Some(a), Some(b)) = (old.metadata.get(key), current.metadata.get(key)) {
                if !a.is_null() && !b.is_null() && a != b {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("conflicting native archive provenance: {key}"),
                    ));
                }
            }
        }
        if is_codex_user_message_mirror(old) && is_codex_user_message_mirror(current) {
            if let (Some(old_turn), Some(current_turn)) = (
                string(old, "provider_turn_id"),
                string(current, "provider_turn_id"),
            ) {
                if old_turn != current_turn {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "conflicting native Codex user-message turn",
                    ));
                }
            }
        }
        let classification_changed = string(current, "input_origin").is_some()
            && string(current, "input_origin") != string(old, "input_origin");
        let metadata = old.metadata.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "archive metadata must be an object",
            )
        })?;
        for key in PROVENANCE_KEYS {
            if let Some(value) = current.metadata.get(*key).filter(|v| !v.is_null()) {
                // `unreported` conveys no new observation.
                if *key == "context_observation"
                    && value == "unreported"
                    && metadata.contains_key(*key)
                {
                    continue;
                }
                metadata.insert((*key).into(), value.clone());
            }
        }
        if classification_changed
            && matches!(string(current, "input_origin"), Some("provider_internal"))
            && current.metadata.get("request_root_id").is_none()
        {
            old.metadata
                .as_object_mut()
                .unwrap()
                .remove("request_root_id");
        }
        if old.turn_id.is_none() {
            old.turn_id = current.turn_id.clone();
        }
    }
    if broker_input {
        // The broker-owned request root remains authoritative, but the
        // provider's explicit turn identity is native evidence needed by the
        // chat projection to join its mirrored user observation.
        if let Some(provider_turn_id) = current
            .metadata
            .get("provider_turn_id")
            .filter(|value| !value.is_null())
        {
            old.metadata["provider_turn_id"] = provider_turn_id.clone();
        }
    }
    // Source fields are observations, not inferred from text or role. They
    // also make a reconciled broker row visible to provider-native consumers.
    for key in [
        "provider_log",
        "log_source",
        "log_path",
        "source_path",
        "provider_session_id",
        "opencode_session_id",
        "step_index",
        "raw_type",
        "provider_step_source",
        "chat_source_ref",
        "chat_source_start",
        "chat_source_end",
        "chat_source_epoch",
    ] {
        if let Some(value) = current.metadata.get(key).filter(|value| !value.is_null()) {
            old.metadata[key] = value.clone();
        }
    }
    if current.metadata["provider_log"] == true {
        old.source = current.source.clone().or(old.source.clone());
        if broker_input {
            old.turn_id = current.turn_id.clone().or(old.turn_id.clone());
        }
    }
    let aliases: BTreeSet<String> = event_identity_ids(old)
        .into_iter()
        .chain(event_identity_ids(current))
        .filter(|id| *id != old.id)
        .map(str::to_owned)
        .collect();
    if !aliases.is_empty() {
        old.metadata["legacy_event_ids"] =
            Value::Array(aliases.into_iter().map(Value::String).collect());
    }
    canonicalize_role(old);
    Ok(())
}

/// Enrich in archive order, retaining original IDs and archive-only history.
/// A current event may collapse two legacy aliases of that same observation.
/// Build a candidate first, so conflicting evidence cannot partially mutate it.
pub(crate) fn refresh_events(
    archived: &mut Vec<AgentChatEvent>,
    current: &[AgentChatEvent],
) -> io::Result<bool> {
    let mut result = archived.clone();
    for observation in current {
        let matches: Vec<usize> = result
            .iter()
            .enumerate()
            .filter_map(|(i, old)| same_observation(old, observation).then_some(i))
            .collect();
        let Some(&first) = matches.first() else {
            continue;
        };
        let mut canonical = result[first].clone();
        for &index in matches.iter().skip(1) {
            let duplicate = &result[index];
            // Preserve archive-only metadata from either historical alias.
            if let (Some(target), Some(source)) = (
                canonical.metadata.as_object_mut(),
                duplicate.metadata.as_object(),
            ) {
                for (key, value) in source {
                    target.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            enrich(&mut canonical, duplicate)?;
        }
        enrich(&mut canonical, observation)?;
        result[first] = canonical;
        for index in matches.into_iter().skip(1).rev() {
            result.remove(index);
        }
    }
    let changed = result != *archived;
    *archived = result;
    Ok(changed)
}

pub(crate) fn changed_observation_ids(
    before_events: &[AgentChatEvent],
    before_records: &[ConversationNarrativeRecord],
    after_events: &[AgentChatEvent],
    after_records: &[ConversationNarrativeRecord],
    observations: &[AgentChatEvent],
) -> HashSet<String> {
    observations
        .iter()
        .filter(|observation| {
            let before_event = before_events
                .iter()
                .find(|event| same_observation(event, observation));
            let after_event = after_events
                .iter()
                .find(|event| same_observation(event, observation));
            let identity_ids = super::event_identity_ids(observation);
            let before_record = record_for_observation(before_records, &identity_ids);
            let after_record = record_for_observation(after_records, &identity_ids);
            before_event != after_event || before_record != after_record
        })
        .map(|observation| observation.id.clone())
        .collect()
}

fn record_for_observation<'a>(
    records: &'a [ConversationNarrativeRecord],
    identity_ids: &[&str],
) -> Option<&'a ConversationNarrativeRecord> {
    records.iter().find(|record| {
        record
            .event_refs
            .iter()
            .any(|event_ref| identity_ids.contains(&event_ref.as_str()))
    })
}

/// Merge a live capture with durable history without text-only deduplication.
/// Logging-disabled callers use this projection without writing an archive.
pub fn merge_current_capture(
    current: Vec<AgentChatEvent>,
    mut archived: Vec<AgentChatEvent>,
) -> io::Result<Vec<AgentChatEvent>> {
    let mut unmatched_current = Vec::with_capacity(current.len());
    let mut matched_generated = std::collections::HashSet::new();
    let mut archived_native_duplicates = BTreeSet::new();
    for event in current {
        let generated = matching_opencode_generated_input(&archived, &event)
            .filter(|index| matched_generated.insert(*index));
        if let Some(generated) = generated {
            let mut canonical = archived[generated].clone();
            enrich(&mut canonical, &event)?;
            archived[generated] = canonical;
            // The normal delivery path archives the generated local echo
            // first, then appends the native DB projection during its first
            // capture. Replaying that archive therefore contains both rows;
            // remove only the already-archived native observation that is
            // bound to this exact current event. The generated row remains
            // the canonical identity after enrichment.
            for (index, archived_event) in archived.iter().enumerate() {
                if index != generated && same_observation(archived_event, &event) {
                    archived_native_duplicates.insert(index);
                }
            }
        } else {
            unmatched_current.push(event);
        }
    }
    for index in archived_native_duplicates.into_iter().rev() {
        archived.remove(index);
    }
    refresh_events(&mut archived, &unmatched_current)?;
    for mut event in unmatched_current {
        if !archived.iter().any(|old| {
            same_observation(old, &event)
                || (old.metadata["provider_log"] != true
                    && event.metadata["provider_log"] != true
                    && old.id == event.id
                    && old.session_id == event.session_id
                    && old.provider == event.provider
                    && old.source == event.source)
        }) {
            canonicalize_role(&mut event);
            archived.push(event);
        }
    }
    collapse_codex_stream_completion_pairs(&mut archived);
    collapse_claude_stream_watch_mirrors(&mut archived);
    collapse_pi_stream_watch_mirrors(&mut archived);
    for (index, event) in archived.iter_mut().enumerate() {
        canonicalize_role(event);
        // Preserve archive-first replay order when a bounded live tail has
        // restarted its sequence counter. This is presentation, not identity.
        event.sequence = Some(index as u64 + 1);
    }
    Ok(archived)
}

/// Collapse Claude's uniquely identified provider/watch mirror while
/// keeping both observation IDs. A missing or ambiguous native match remains
/// visible.
fn collapse_claude_stream_watch_mirrors(events: &mut Vec<AgentChatEvent>) {
    let mut removed = BTreeSet::new();

    for mirror_index in 0..events.len() {
        if removed.contains(&mirror_index) || !is_claude_watch_mirror(&events[mirror_index]) {
            continue;
        }
        let native_matches: Vec<usize> = (0..events.len())
            .filter(|&native_index| {
                native_index != mirror_index
                    && !removed.contains(&native_index)
                    && is_claude_stream_watch_pair(&events[mirror_index], &events[native_index])
            })
            .collect();
        if native_matches.len() != 1 {
            continue;
        }
        let native_index = native_matches[0];
        let mirror_matches: Vec<usize> = (0..events.len())
            .filter(|&candidate_index| {
                candidate_index != native_index
                    && !removed.contains(&candidate_index)
                    && is_claude_stream_watch_pair(&events[candidate_index], &events[native_index])
            })
            .collect();
        if mirror_matches.len() != 1 {
            continue;
        }

        let mut canonical = events[native_index].clone();
        retain_provider_observation_ids(&mut canonical, &events[mirror_index]);
        events[native_index] = canonical;
        removed.insert(mirror_index);
    }

    if !removed.is_empty() {
        *events = events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

fn is_claude_watch_mirror(event: &AgentChatEvent) -> bool {
    event.provider == "claude"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && event.source.as_deref() == Some("stream_json")
        && event.metadata["provider_log"] != true
        && event.metadata["provider_source"] == "event"
        && event
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| !turn_id.trim().is_empty())
}

fn is_claude_stream_watch_pair(mirror: &AgentChatEvent, native: &AgentChatEvent) -> bool {
    is_claude_watch_mirror(mirror)
        && native.provider == "claude"
        && native.kind == AgentChatEventKind::Message
        && native.role == Some(AgentChatRole::Assistant)
        && native.source.as_deref() == Some("stream_json")
        && native.metadata["provider_log"] == true
        && string(native, "log_path").is_some()
        && mirror.session_id == native.session_id
        && mirror.turn_id == native.turn_id
        && mirror.text.as_deref().is_some_and(|text| !text.is_empty())
        && mirror.text.as_deref() == native.text.as_deref()
}

/// Collapse only Codex's identityless assistant stream mirror when the
/// identified final response proves it is the same native turn and source.
/// Older rows without these bindings remain untouched.
fn collapse_codex_stream_completion_pairs(events: &mut Vec<AgentChatEvent>) {
    let mut removed = BTreeSet::new();
    let completions = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| is_codex_final_completion(event).then_some(index))
        .collect::<Vec<_>>();

    for completion_index in completions {
        let Some(members) = codex_completion_group_members(events, completion_index) else {
            continue;
        };
        if members.iter().any(|index| removed.contains(index)) {
            continue;
        }
        let mut canonical = events[completion_index].clone();
        for mirror_index in members
            .into_iter()
            .filter(|index| *index != completion_index)
        {
            retain_provider_observation_ids(&mut canonical, &events[mirror_index]);
            removed.insert(mirror_index);
        }
        events[completion_index] = canonical;
    }

    if !removed.is_empty() {
        *events = events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

fn is_codex_assistant_mirror(event: &AgentChatEvent) -> bool {
    event.provider == "codex"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && event.source.as_deref() == Some("event_msg")
        && event.turn_id.is_none()
        && event.metadata["provider_log"] == true
        && event.metadata["provider_source"] != "event"
}

fn is_codex_live_watch_observation(event: &AgentChatEvent) -> bool {
    event.provider == "codex"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && matches!(event.source.as_deref(), Some("event_msg" | "response_item"))
        && event.metadata["provider_log"] == true
        && event.metadata["provider_source"] == "event"
        && string(event, "provider_session_id").is_some()
        && string(event, "log_path").is_some()
        && string(event, "provider_turn_id").is_some()
        && string(event, "provider_phase") != Some("final_answer")
}

fn is_codex_final_completion(event: &AgentChatEvent) -> bool {
    event.provider == "codex"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && event.source.as_deref() == Some("response_item")
        && event
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| !turn_id.is_empty())
        && event.metadata["provider_log"] == true
        && string(event, "provider_phase") == Some("final_answer")
}

fn same_codex_assistant_text(first: &AgentChatEvent, second: &AgentChatEvent) -> bool {
    match (first.text.as_deref(), second.text.as_deref()) {
        (Some(first_text), Some(second_text)) => {
            !first_text.is_empty() && first_text == second_text
        }
        (None, None) => string(first, "codex_assistant_text_sha256")
            .zip(string(second, "codex_assistant_text_sha256"))
            .is_some_and(|(first_hash, second_hash)| first_hash == second_hash),
        _ => false,
    }
}

pub(crate) fn codex_live_watch_observation_pair(
    first: &AgentChatEvent,
    second: &AgentChatEvent,
) -> bool {
    let sources_pair = matches!(
        (first.source.as_deref(), second.source.as_deref()),
        (Some("event_msg"), Some("response_item")) | (Some("response_item"), Some("event_msg"))
    );
    sources_pair
        && is_codex_live_watch_observation(first)
        && is_codex_live_watch_observation(second)
        && first.session_id == second.session_id
        && string(first, "provider_session_id") == string(second, "provider_session_id")
        && string(first, "log_path") == string(second, "log_path")
        && string(first, "provider_turn_id") == string(second, "provider_turn_id")
        && same_codex_assistant_text(first, second)
}

/// Return the uniquely bound native mirror and optional live watch pair for a
/// final Codex answer. Every candidate must map to this one completion.
pub(crate) fn codex_completion_group_members(
    events: &[AgentChatEvent],
    completion_index: usize,
) -> Option<Vec<usize>> {
    let completion = events.get(completion_index)?;
    if !is_codex_final_completion(completion) {
        return None;
    }
    let mirrors = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            (index != completion_index && is_codex_stream_completion_pair(event, completion))
                .then_some(index)
        })
        .collect::<Vec<_>>();
    for mirror_index in &mirrors {
        let completion_count = events
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                *index != *mirror_index
                    && is_codex_final_completion(candidate)
                    && is_codex_stream_completion_pair(&events[*mirror_index], candidate)
            })
            .count();
        if completion_count != 1 {
            return None;
        }
    }

    let native_mirrors = mirrors
        .iter()
        .copied()
        .filter(|index| is_codex_assistant_mirror(&events[*index]))
        .collect::<Vec<_>>();
    if native_mirrors.len() != 1 {
        return None;
    }

    let watch_mirrors = mirrors
        .iter()
        .copied()
        .filter(|index| is_codex_live_watch_observation(&events[*index]))
        .collect::<Vec<_>>();
    if !watch_mirrors.is_empty() {
        let watch_messages = watch_mirrors
            .iter()
            .filter(|index| events[**index].source.as_deref() == Some("event_msg"))
            .copied()
            .collect::<Vec<_>>();
        let watch_responses = watch_mirrors
            .iter()
            .filter(|index| events[**index].source.as_deref() == Some("response_item"))
            .copied()
            .collect::<Vec<_>>();
        if watch_messages.len() != 1
            || watch_responses.len() != 1
            || !codex_live_watch_observation_pair(
                &events[watch_messages[0]],
                &events[watch_responses[0]],
            )
        {
            return None;
        }
    }

    let mut members = mirrors;
    members.push(completion_index);
    Some(members)
}

fn is_codex_stream_completion_pair(mirror: &AgentChatEvent, completion: &AgentChatEvent) -> bool {
    if !(is_codex_assistant_mirror(mirror) || is_codex_live_watch_observation(mirror))
        || !is_codex_final_completion(completion)
        || mirror.session_id != completion.session_id
        || !same_codex_assistant_text(mirror, completion)
    {
        return false;
    }
    let same_log_path = string(mirror, "log_path")
        .zip(string(completion, "log_path"))
        .is_some_and(|(mirror_path, completion_path)| mirror_path == completion_path);
    let same_provider_turn = string(mirror, "provider_turn_id")
        .zip(string(completion, "provider_turn_id"))
        .is_some_and(|(mirror_turn, completion_turn)| mirror_turn == completion_turn);
    let provider_sessions_are_compatible = match (
        string(mirror, "provider_session_id"),
        string(completion, "provider_session_id"),
    ) {
        (Some(mirror_session), Some(completion_session)) => mirror_session == completion_session,
        _ => true,
    };
    same_log_path && same_provider_turn && provider_sessions_are_compatible
}

/// A mirror cannot establish ownership when distinct identified completions
/// match it. Check all known observations, including the pending batch, before
/// assigning a durable narrative so iteration order cannot choose an owner.
pub(super) fn codex_unique_stream_completion_pair<'a>(
    first: &AgentChatEvent,
    second: &AgentChatEvent,
    candidates: impl IntoIterator<Item = &'a AgentChatEvent>,
) -> bool {
    let mirror = if is_codex_stream_completion_pair(first, second) {
        first
    } else if is_codex_stream_completion_pair(second, first) {
        second
    } else {
        return false;
    };
    let mut completion_id = None;
    for candidate in candidates {
        if is_codex_stream_completion_pair(mirror, candidate) {
            if completion_id.is_some_and(|id| id != candidate.id.as_str()) {
                return false;
            }
            completion_id = Some(candidate.id.as_str());
        }
    }
    completion_id.is_some()
}

/// Collapse Pi's uniquely bound session JSONL watcher mirror while retaining
/// both observation IDs. Unbound or ambiguous observations remain visible.
fn collapse_pi_stream_watch_mirrors(events: &mut Vec<AgentChatEvent>) {
    let mut removed = BTreeSet::new();

    for mirror_index in 0..events.len() {
        if removed.contains(&mirror_index) || !is_pi_watch_mirror(&events[mirror_index]) {
            continue;
        }
        let native_matches: Vec<usize> = (0..events.len())
            .filter(|&native_index| {
                native_index != mirror_index
                    && !removed.contains(&native_index)
                    && is_pi_stream_watch_pair(&events[mirror_index], &events[native_index])
            })
            .collect();
        if native_matches.len() != 1 {
            continue;
        }
        let native_index = native_matches[0];
        let mirror_matches: Vec<usize> = (0..events.len())
            .filter(|&candidate_index| {
                candidate_index != native_index
                    && !removed.contains(&candidate_index)
                    && is_pi_stream_watch_pair(&events[candidate_index], &events[native_index])
            })
            .collect();
        if mirror_matches.len() != 1 {
            continue;
        }

        let mut canonical = events[native_index].clone();
        retain_provider_observation_ids(&mut canonical, &events[mirror_index]);
        events[native_index] = canonical;
        removed.insert(mirror_index);
    }

    if !removed.is_empty() {
        *events = events
            .drain(..)
            .enumerate()
            .filter_map(|(index, event)| (!removed.contains(&index)).then_some(event))
            .collect();
    }
}

fn is_pi_watch_mirror(event: &AgentChatEvent) -> bool {
    event.provider == "pi"
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
        && event.source.as_deref() == Some("session_jsonl")
        && event.metadata["provider_log"] == true
        && string(event, "provider_session_id").is_some()
        && string(event, "log_path").is_some()
        && event
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| !turn_id.trim().is_empty())
}

fn is_pi_stream_watch_pair(mirror: &AgentChatEvent, native: &AgentChatEvent) -> bool {
    is_pi_watch_mirror(mirror)
        && native.provider == "pi"
        && native.kind == AgentChatEventKind::Message
        && native.role == Some(AgentChatRole::Assistant)
        && native.source.as_deref() == Some("message")
        && native.metadata["provider_log"] == true
        && native
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| !turn_id.trim().is_empty())
        && mirror.session_id == native.session_id
        && mirror.turn_id == native.turn_id
        && string(mirror, "log_path")
            .zip(string(native, "log_path"))
            .is_some_and(|(mirror_path, native_path)| mirror_path == native_path)
        && mirror.text.as_deref().is_some_and(|text| !text.is_empty())
        && mirror.text.as_deref() == native.text.as_deref()
}

fn retain_provider_observation_ids(canonical: &mut AgentChatEvent, duplicate: &AgentChatEvent) {
    let mut ids = provider_observation_ids(canonical);
    for id in provider_observation_ids(duplicate) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    canonical.metadata["provider_observation_ids"] = serde_json::json!(ids);

    if canonical.source.as_deref() == Some("response_item")
        && duplicate.source.as_deref() == Some("event_msg")
    {
        let mut roots = canonical.metadata["provider_mirror_request_root_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mirror_roots = duplicate.metadata["provider_mirror_request_root_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(string(duplicate, "request_root_id"));
        for root in mirror_roots {
            if string(canonical, "request_root_id") != Some(root)
                && !roots.iter().any(|value| value.as_str() == Some(root))
            {
                roots.push(serde_json::json!(root));
            }
        }
        if !roots.is_empty() {
            canonical.metadata["provider_mirror_request_root_ids"] =
                serde_json::Value::Array(roots);
        }
    }
}

fn provider_observation_ids(event: &AgentChatEvent) -> Vec<String> {
    let mut ids = event
        .metadata
        .get("provider_observation_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !ids.iter().any(|id| id == &event.id) {
        ids.push(event.id.clone());
    }
    ids
}

/// Reconcile the live OpenCode database projection with its already archived
/// generated input when archive logging is disabled. Text alone is not an
/// identity key: the generated marker, request ownership, provider session,
/// source path, and a unique candidate are all required. A generated input
/// may be unbound when the authoritative provider source was unavailable at
/// delivery time; it is eligible for late binding only when that candidate is
/// otherwise uniquely owned by the native session. Ambiguous repeated prompts
/// therefore remain separate observations.
fn matching_opencode_generated_input(
    archived: &[AgentChatEvent],
    native: &AgentChatEvent,
) -> Option<usize> {
    if native.provider != "opencode"
        || native.source.as_deref() != Some("opencode_db")
        || native.metadata["provider_log"] != true
        || native.kind != AgentChatEventKind::Message
        || native.role != Some(AgentChatRole::User)
        || matches!(
            string(native, "input_origin"),
            Some("provider_internal" | "context_injection")
        )
        || string(native, "opencode_session_id").is_none()
        || string(native, "source_path").is_none()
    {
        return None;
    }
    let native_text = native.text.as_deref()?;
    if native_text.is_empty() {
        return None;
    }
    let native_session = string(native, "opencode_session_id")?;
    let provider_source_key = format!("opencode:session:{native_session}");
    let matches: Vec<usize> = archived
        .iter()
        .enumerate()
        .filter_map(|(index, generated)| {
            let has_expected_binding =
                generated.turn_id.as_deref() == Some(provider_source_key.as_str());
            let is_unbound_broker_echo = generated.turn_id.is_none()
                && generated.source.is_none()
                && generated.metadata["provider_log"] != true;
            (generated.metadata["generated"] == true
                && generated.session_id == native.session_id
                && generated.provider == "opencode"
                && generated.kind == AgentChatEventKind::Message
                && generated.role == Some(AgentChatRole::User)
                && (has_expected_binding || is_unbound_broker_echo)
                && matches!(
                    string(generated, "input_origin"),
                    Some("human_input" | "agent_input")
                )
                && string(generated, "input_purpose") == Some("request")
                && string(generated, "request_root_id")
                    .is_some_and(|root| root.starts_with("wardian:input:"))
                && generated.text.as_deref().is_some_and(|generated_text| {
                    generated_text.as_bytes() == native_text.as_bytes()
                }))
            .then_some(index)
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

pub(super) fn refresh_records(
    records: &mut Vec<ConversationNarrativeRecord>,
    events: &[AgentChatEvent],
) {
    for event in events {
        let claude_raw_line = event.provider == "claude" && event.metadata["provider_log"] == true;
        let (matches, ids) = if claude_raw_line {
            let exact_matches: Vec<usize> = records
                .iter()
                .enumerate()
                .filter_map(|(i, record)| record.event_refs.contains(&event.id).then_some(i))
                .collect();
            // Claude raw-line IDs retain the provider observation boundary;
            // its legacy aliases can collide across distinct log positions.
            if exact_matches.len() > 1 {
                continue;
            }
            if !exact_matches.is_empty() {
                (exact_matches, vec![event.id.as_str()])
            } else {
                let ids = event_identity_ids(event);
                let matches = records
                    .iter()
                    .enumerate()
                    .filter_map(|(i, record)| {
                        record
                            .event_refs
                            .iter()
                            .any(|id| ids.contains(&id.as_str()))
                            .then_some(i)
                    })
                    .collect::<Vec<_>>();
                if matches.len() > 1 {
                    continue;
                }
                (matches, ids)
            }
        } else {
            let ids = event_identity_ids(event);
            let matches = records
                .iter()
                .enumerate()
                .filter_map(|(i, record)| {
                    record
                        .event_refs
                        .iter()
                        .any(|id| ids.contains(&id.as_str()))
                        .then_some(i)
                })
                .collect();
            (matches, ids)
        };
        let Some(&first) = matches.first() else {
            continue;
        };
        let Some(projection) = narrative_from_chat_event(event, records[first].seq) else {
            continue;
        };
        let mut canonical = records[first].clone();
        canonical.turn_id = projection.turn_id.or(canonical.turn_id);
        if completed_native_tool(event) {
            // Also repairs a narrative left stale by failure after event publish.
            // Preserve narrative sequence/time/source refs and outcome status.
            if let Some(text) = event.text.as_ref() {
                if canonical.text.as_ref() != Some(text) {
                    canonical.text = Some(text.clone());
                    canonical.excerpt = None;
                    canonical.artifact_refs.clear();
                }
            } else if let Some(refs) = event.metadata["text_artifact_refs"].as_array() {
                canonical.text = None;
                canonical.excerpt = event.metadata["text_excerpt"].as_str().map(str::to_owned);
                canonical.artifact_refs = refs
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect();
            }
        }
        // Broker-authored delivery provenance remains authoritative. Native
        // observation aliases enrich it without turning an agent input human.
        if !canonical
            .event_refs
            .iter()
            .any(|id| id.starts_with("generated:"))
        {
            canonical.role = projection.role;
            canonical.speaker_type = projection.speaker_type;
            canonical.input_origin = projection.input_origin;
            canonical.input_purpose = projection.input_purpose;
            if let Some(root) = string(event, "request_root_id") {
                canonical.request_root_id = Some(root.to_string());
            } else if string(event, "input_origin") == Some("provider_internal") {
                canonical.request_root_id = None;
            }
            canonical.causal_ref = projection.causal_ref.or(canonical.causal_ref);
        }
        for id in ids {
            if !canonical.event_refs.iter().any(|old| old == id) {
                canonical.event_refs.push(id.into());
            }
        }
        for &index in matches.iter().skip(1) {
            let duplicate = &records[index];
            for (target, source) in [
                (&mut canonical.event_refs, &duplicate.event_refs),
                (&mut canonical.source_refs, &duplicate.source_refs),
                (&mut canonical.artifact_refs, &duplicate.artifact_refs),
            ] {
                for value in source {
                    if !target.contains(value) {
                        target.push(value.clone());
                    }
                }
            }
        }
        records[first] = canonical;
        for index in matches.into_iter().skip(1).rev() {
            records.remove(index);
        }
    }
}

/// Reuse the broker's already-persisted event_refs reconciliation, not prompt
/// text. The ordinary generated row keeps its ID and broker input provenance,
/// while exposing the native source observation that was previously hidden.
pub(super) fn bind_delivered_inputs(
    events: &mut Vec<AgentChatEvent>,
    records: &[ConversationNarrativeRecord],
) -> io::Result<bool> {
    let generated_ids = events
        .iter()
        .filter(|event| {
            event.metadata["generated"] == true && event.kind == AgentChatEventKind::Message
        })
        .map(|event| event.id.clone())
        .collect::<HashSet<_>>();
    if generated_ids.is_empty() {
        return Ok(false);
    }
    let before = events.clone();
    for record in records {
        // Binding only enriches an existing generated message or removes a
        // native observation. It never creates a new generated ID, so this
        // initial set is a conservative filter even after an earlier removal.
        if !record
            .event_refs
            .iter()
            .any(|event_ref| generated_ids.contains(event_ref))
        {
            continue;
        }
        let generated = events.iter().position(|event| {
            event.metadata["generated"] == true
                && event.kind == AgentChatEventKind::Message
                && record.event_refs.contains(&event.id)
        });
        let Some(generated) = generated else {
            continue;
        };
        let candidates: Vec<usize> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                (index != generated
                    && !event.metadata["chat_source_ref"].is_string()
                    && record.event_refs.contains(&event.id)
                    && event.session_id == events[generated].session_id
                    && event.provider == events[generated].provider
                    && event.metadata["provider_log"] == true
                    && event.kind == AgentChatEventKind::Message
                    && event.role == Some(AgentChatRole::User)
                    && !matches!(
                        string(event, "input_origin"),
                        Some("provider_internal" | "context_injection")
                    )
                    && (string(event, "log_path").is_some()
                        || string(event, "provider_session_id").is_some()))
                .then_some(index)
            })
            .collect();
        let [native] = candidates.as_slice() else {
            continue;
        };
        let observation = events[*native].clone();
        enrich(&mut events[generated], &observation)?;
        events.remove(*native);
    }
    Ok(*events != before)
}

#[cfg(test)]
pub(crate) use test_support::codex_stream_completion_pair;

#[cfg(test)]
mod test_support {
    use super::{is_codex_stream_completion_pair, AgentChatEvent};

    pub(crate) fn codex_stream_completion_pair(
        first: &AgentChatEvent,
        second: &AgentChatEvent,
    ) -> bool {
        is_codex_stream_completion_pair(first, second)
            || is_codex_stream_completion_pair(second, first)
    }
}
