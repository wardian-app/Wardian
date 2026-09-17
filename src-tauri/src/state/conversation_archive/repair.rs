use std::{collections::HashSet, io, path::Path};

use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, read_jsonl_records, write_json_atomic,
    write_jsonl_atomic, ConversationNarrativeRecord, ConversationRecordKind,
    ConversationSourceRecord, ConversationTurnRecord,
};
use wardian_core::models::chat::AgentChatEvent;
use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};

use super::{
    apply_archive_summary_to_manifest, archive_summary, artifact_count_for_records,
    event_identity_ids, excerpt_from_record, generated_sources_from_record,
    index_entry_from_manifest, index_path, materialize_record_text, narrative_from_chat_event,
    open_manifest, provenance, source_record_from_chat_event, ActiveConversationHandle,
    ConversationArchiveContext,
};

pub(super) struct PendingChatPublication {
    pub event: Option<AgentChatEvent>,
    pub source: Option<ConversationSourceRecord>,
    pub record: ConversationNarrativeRecord,
}

pub(super) struct RecoveredObservation {
    pub record: ConversationNarrativeRecord,
    pub sources: Vec<ConversationSourceRecord>,
}

pub(super) fn matching_event_index(
    existing: &[AgentChatEvent],
    current: &AgentChatEvent,
) -> io::Result<Option<usize>> {
    let current_ids = event_identity_ids(current);
    unique_match(
        existing.iter().enumerate().filter_map(|(index, old)| {
            let shares_identity = event_identity_ids(old)
                .iter()
                .any(|id| !id.is_empty() && current_ids.iter().any(|current_id| current_id == id));
            if !shares_identity
                || old.session_id != current.session_id
                || old.provider != current.provider
                || old.kind != current.kind
            {
                return None;
            }
            if old.metadata["provider_log"] == true || current.metadata["provider_log"] == true {
                provenance::same_observation(old, current).then_some(index)
            } else {
                Some(index)
            }
        }),
        &format!("event identity {}", current.id),
    )
}

pub(super) fn matching_record_index(
    records: &[ConversationNarrativeRecord],
    current: &AgentChatEvent,
) -> io::Result<Option<usize>> {
    let ids = event_identity_ids(current);
    unique_match(
        records.iter().enumerate().filter_map(|(index, record)| {
            record
                .event_refs
                .iter()
                .any(|event_ref| ids.contains(&event_ref.as_str()))
                .then_some(index)
        }),
        &format!("narrative ownership for event {}", current.id),
    )
}

fn unique_match(
    matches: impl IntoIterator<Item = usize>,
    description: &str,
) -> io::Result<Option<usize>> {
    let mut matched = None;
    for index in matches {
        if matched.replace(index).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ambiguous {description}: multiple durable owners"),
            ));
        }
    }
    Ok(matched)
}

pub(super) fn coalesce_batch_observations(
    events: &[AgentChatEvent],
) -> io::Result<Vec<AgentChatEvent>> {
    let mut canonical = Vec::with_capacity(events.len());
    for event in events {
        let current_ids = event_identity_ids(event);
        let matches = canonical
            .iter()
            .enumerate()
            .filter_map(|(index, previous)| {
                event_identity_ids(previous)
                    .iter()
                    .any(|id| {
                        !id.is_empty() && current_ids.iter().any(|current_id| current_id == id)
                    })
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ambiguous same-batch identity for event {}", event.id),
            ));
        }
        let Some(index) = matches.first().copied() else {
            canonical.push(event.clone());
            continue;
        };
        if canonical[index] == *event {
            continue;
        }
        if !provenance::same_observation(&canonical[index], event) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("conflicting same-batch identity for event {}", event.id),
            ));
        }
        let mut merged = vec![canonical[index].clone()];
        provenance::refresh_events(&mut merged, std::slice::from_ref(event))?;
        canonical[index] = merged.pop().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("same-batch enrichment removed event {}", event.id),
            )
        })?;
    }
    Ok(canonical)
}

pub(super) fn is_bound_native_delivery(
    record: &ConversationNarrativeRecord,
    event: &AgentChatEvent,
) -> bool {
    event.metadata["provider_log"] == true
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && record
            .event_refs
            .iter()
            .any(|event_ref| event_ref.starts_with("generated:"))
}

pub(super) fn recover_unlinked_observations(
    conversation_id: &str,
    directory: &Path,
    context: &ConversationArchiveContext,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
    recover_only_ids: Option<&HashSet<String>>,
    next_seq: &mut u64,
) -> io::Result<Vec<RecoveredObservation>> {
    let mut occupied_sequences = HashSet::new();
    for record in records {
        if !occupied_sequences.insert(record.seq) {
            return Err(recoverable_recovery_error(format!(
                "duplicate narrative sequence {} prevents recovery",
                record.seq
            )));
        }
    }
    let mut recovery_seq = first_missing_sequence(&occupied_sequences)?;
    let mut known_sources: Option<Vec<ConversationSourceRecord>> = None;
    let mut recovered_event_ids = HashSet::new();
    let mut recovered = Vec::new();

    for event in events {
        if !event.id.trim().is_empty()
            && recover_only_ids.is_some_and(|ids| {
                !event_identity_ids(event)
                    .iter()
                    .any(|event_id| ids.contains(*event_id))
            })
        {
            continue;
        }
        if event.id.trim().is_empty() {
            if event.metadata["generated"] == true
                || narrative_from_chat_event(event, recovery_seq).is_some()
            {
                return Err(recoverable_recovery_error(
                    "durable observation lacks an event identity".to_string(),
                ));
            }
            continue;
        }
        if matching_record_index(records, event)?.is_some() {
            continue;
        }
        let event_ids = event_identity_ids(event);
        if event_ids
            .iter()
            .any(|event_id| recovered_event_ids.contains(*event_id))
        {
            return Err(recoverable_recovery_error(format!(
                "durable observation {} has an ambiguous duplicate identity",
                event.id
            )));
        }

        let is_generated = event.metadata["generated"] == true;
        let mut record = if is_generated {
            let payload = event.metadata.get("archive_record").ok_or_else(|| {
                recoverable_recovery_error(format!(
                    "generated observation {} lacks its reconstructable archive payload",
                    event.id
                ))
            })?;
            let mut record: ConversationNarrativeRecord = serde_json::from_value(payload.clone())
                .map_err(|error| {
                recoverable_recovery_error(format!(
                    "generated observation {} has invalid archive payload: {error}",
                    event.id
                ))
            })?;
            let expected_id = format!("generated:{conversation_id}:{}", record.seq);
            if event.id != expected_id {
                return Err(recoverable_recovery_error(format!(
                    "generated observation {} does not match its archived sequence",
                    event.id
                )));
            }
            if record.seq != recovery_seq {
                return Err(recoverable_recovery_error(format!(
                    "generated observation {} is out of recovery sequence order: expected {}, found {}",
                    event.id, recovery_seq, record.seq
                )));
            }
            if !record
                .event_refs
                .iter()
                .any(|event_ref| event_ref == &event.id)
            {
                record.event_refs.push(event.id.clone());
            }
            record
        } else {
            let Some(mut record) = narrative_from_chat_event(event, recovery_seq) else {
                continue;
            };
            restore_event_text_metadata(event, &mut record)?;
            record.event_refs = vec![event.id.clone()];
            record
        };

        if is_generated {
            validate_reconstructable_record(event, &record)?;
        }
        if !occupied_sequences.insert(record.seq) {
            return Err(recoverable_recovery_error(format!(
                "durable observation {} collides with an existing narrative sequence {}",
                event.id, record.seq
            )));
        }

        let sources = if is_generated {
            generated_sources_from_record(context, &mut record)
        } else {
            source_record_from_chat_event(event, record.seq)
                .into_iter()
                .collect::<Vec<_>>()
        };
        if !is_generated && !sources.is_empty() {
            record.source_refs = sources
                .iter()
                .map(|source| source.source_id.clone())
                .collect();
        }
        for source in &sources {
            if !record
                .source_refs
                .iter()
                .any(|source_ref| source_ref == &source.source_id)
            {
                record.source_refs.push(source.source_id.clone());
            }
            let known =
                known_sources.get_or_insert(read_jsonl_records(&directory.join("sources.jsonl"))?);
            if !source_row_is_published(known, source)? {
                known.push(source.clone());
            }
        }

        recovered_event_ids.extend(event_ids.into_iter().map(ToString::to_string));
        recovery_seq = first_missing_sequence(&occupied_sequences)?;
        *next_seq = (*next_seq).max(record.seq.saturating_add(1));
        recovered.push(RecoveredObservation { record, sources });
    }

    let has_generated_history = records.iter().any(|record| {
        record
            .event_refs
            .iter()
            .any(|event_ref| event_ref.starts_with("generated:"))
    }) || events
        .iter()
        .any(|event| event.metadata["generated"] == true);
    if has_generated_history || !recovered.is_empty() {
        let mut all_sequences = records.iter().map(|record| record.seq).collect::<Vec<_>>();
        all_sequences.extend(recovered.iter().map(|observation| observation.record.seq));
        validate_sequence_prefix(all_sequences)?;
    }

    for observation in &mut recovered {
        materialize_record_text(directory, &mut observation.record)?;
    }
    Ok(recovered)
}

fn first_missing_sequence(occupied: &HashSet<u64>) -> io::Result<u64> {
    let mut candidate = 1;
    while occupied.contains(&candidate) {
        candidate = candidate.checked_add(1).ok_or_else(|| {
            recoverable_recovery_error("narrative sequence space is exhausted".to_string())
        })?;
    }
    Ok(candidate)
}

fn validate_sequence_prefix(sequences: impl IntoIterator<Item = u64>) -> io::Result<()> {
    let mut occupied = HashSet::new();
    let mut maximum = 0;
    for sequence in sequences {
        if !occupied.insert(sequence) {
            return Err(recoverable_recovery_error(format!(
                "duplicate narrative sequence {} prevents recovery",
                sequence
            )));
        }
        maximum = maximum.max(sequence);
    }
    for expected in 1..=maximum {
        if !occupied.contains(&expected) {
            return Err(recoverable_recovery_error(format!(
                "narrative sequence gap before {} prevents recovery",
                expected
            )));
        }
    }
    Ok(())
}

pub(super) fn publish_recovered_observations(
    conversation_path: &Path,
    sources_path: &Path,
    records: &mut Vec<ConversationNarrativeRecord>,
    observations: Vec<RecoveredObservation>,
) -> io::Result<usize> {
    let count = observations.len();
    if count == 0 {
        return Ok(0);
    }
    let mut repaired_records = records.clone();
    repaired_records.extend(
        observations
            .iter()
            .map(|observation| observation.record.clone()),
    );
    validate_sequence_prefix(repaired_records.iter().map(|record| record.seq))?;

    let mut cached_sources = None;
    for observation in &observations {
        for source in &observation.sources {
            append_source_if_needed(sources_path, &mut cached_sources, source, true)?;
        }
    }
    repaired_records.sort_by_key(|record| record.seq);
    records.clear();
    records.extend(repaired_records);
    write_jsonl_atomic(conversation_path, records)?;
    Ok(count)
}

fn restore_event_text_metadata(
    event: &AgentChatEvent,
    record: &mut ConversationNarrativeRecord,
) -> io::Result<()> {
    if let Some(refs) = event.metadata.get("text_artifact_refs") {
        let Some(refs) = refs.as_array() else {
            return Err(recoverable_recovery_error(format!(
                "observation {} has malformed text artifact references",
                event.id
            )));
        };
        let mut artifact_refs = Vec::with_capacity(refs.len());
        for reference in refs {
            let Some(reference) = reference.as_str() else {
                return Err(recoverable_recovery_error(format!(
                    "observation {} has a non-string text artifact reference",
                    event.id
                )));
            };
            artifact_refs.push(reference.to_string());
        }
        if !artifact_refs.is_empty() {
            record.text = None;
            record.artifact_refs = artifact_refs;
        }
    }
    if let Some(excerpt) = event.metadata["text_excerpt"].as_str() {
        record.excerpt = Some(excerpt.to_string());
    }
    validate_reconstructable_record(event, record)
}

fn validate_reconstructable_record(
    event: &AgentChatEvent,
    record: &ConversationNarrativeRecord,
) -> io::Result<()> {
    if record.kind == ConversationRecordKind::Message
        && record.text.is_none()
        && record.artifact_refs.is_empty()
        && record.excerpt.is_none()
    {
        return Err(recoverable_recovery_error(format!(
            "observation {} lacks narrative text or artifact reconstruction data",
            event.id
        )));
    }
    Ok(())
}

fn recoverable_recovery_error(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, message)
}

pub(super) fn rebuild_derived_projections(
    agent_id: &str,
    directory: &Path,
    context: &ConversationArchiveContext,
    handle: &ActiveConversationHandle,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
) -> io::Result<()> {
    let Some(first_record) = records.first() else {
        return Ok(());
    };
    let Some(last_record) = records.last() else {
        return Ok(());
    };
    let mut all_events = events.to_vec();
    if provenance::bind_delivered_inputs(&mut all_events, records)? {
        write_jsonl_atomic(&directory.join("events.jsonl"), &all_events)?;
    }
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&directory.join("sources.jsonl"))?;
    let turns: Vec<ConversationTurnRecord> = super::derive_turn_records_with_context(
        &handle.conversation_id,
        records,
        &all_events,
        &sources,
        true,
        Some(&context.provider),
        &context.provider_session_ids,
    );
    write_jsonl_atomic(&directory.join("turns.jsonl"), &turns)?;
    let summary = archive_summary(records, &turns, &sources);
    let mut manifest = open_manifest(
        context,
        &handle.conversation_id,
        first_record.at.clone(),
        last_record.at.clone(),
    );
    apply_archive_summary_to_manifest(&mut manifest, &summary);
    write_json_atomic(&directory.join("manifest.json"), &manifest)?;
    append_index_upsert(
        &index_path(agent_id)?,
        &index_entry_from_manifest(
            &manifest,
            None,
            excerpt_from_record(first_record),
            excerpt_from_record(last_record),
            records.len() as u64,
            artifact_count_for_records(records.iter()),
        ),
    )?;
    Ok(())
}

pub(super) fn append_source_if_needed(
    path: &Path,
    cached: &mut Option<Vec<ConversationSourceRecord>>,
    source: &ConversationSourceRecord,
    repair_existing: bool,
) -> io::Result<bool> {
    if repair_existing {
        let sources = cached.get_or_insert(read_jsonl_records(path)?);
        if source_row_is_published(sources, source)? {
            return Ok(false);
        }
    } else if let Some(sources) = cached.as_ref() {
        if source_row_is_published(sources, source)? {
            return Ok(false);
        }
    }

    append_jsonl_record(path, source)?;
    if let Some(sources) = cached.as_mut() {
        sources.push(source.clone());
    }
    Ok(true)
}

fn source_row_is_published(
    sources: &[ConversationSourceRecord],
    expected: &ConversationSourceRecord,
) -> io::Result<bool> {
    if sources.iter().any(|source| source == expected) {
        return Ok(true);
    }

    if expected.source_id.starts_with("agent:") {
        for source in sources
            .iter()
            .filter(|source| source.source_id == expected.source_id)
        {
            if source.provider != expected.provider
                || source.provider_session_id != expected.provider_session_id
                || source.source_kind != expected.source_kind
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("conflicting shared source identity: {}", expected.source_id),
                ));
            }
        }
        return Ok(false);
    }

    if sources
        .iter()
        .any(|source| source.source_id == expected.source_id)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("conflicting source identity: {}", expected.source_id),
        ));
    }
    Ok(false)
}
