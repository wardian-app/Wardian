use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
};

use wardian_core::conversations::{
    append_index_upsert, append_jsonl_record, read_jsonl_records, write_json_atomic,
    write_jsonl_atomic, ConversationInputOrigin, ConversationNarrativeRecord,
    ConversationRecordKind, ConversationSourceRecord, ConversationTurnRecord,
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
    pub events: Vec<AgentChatEvent>,
    pub sources: Vec<ConversationSourceRecord>,
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

/// Collapse request narratives that an older archive recorded separately
/// before both Codex user-input observations were available.
pub(super) fn reconcile_codex_user_mirror_records(
    records: &mut Vec<ConversationNarrativeRecord>,
    events: &mut [AgentChatEvent],
) -> io::Result<u64> {
    let unique_sequences = records
        .iter()
        .map(|record| record.seq)
        .collect::<HashSet<_>>();
    if unique_sequences.len() != records.len() || unique_sequences.contains(&0) {
        return Ok(0);
    }

    let mut event_owners = HashMap::<&str, Vec<usize>>::new();
    for (record_index, record) in records.iter().enumerate() {
        let mut seen = HashSet::new();
        for event_ref in &record.event_refs {
            if seen.insert(event_ref.as_str()) {
                event_owners
                    .entry(event_ref.as_str())
                    .or_default()
                    .push(record_index);
            }
        }
    }

    let mut requests_by_turn = HashMap::<&str, Vec<usize>>::new();
    let mut mirrors_by_turn = HashMap::<&str, Vec<usize>>::new();
    for (index, event) in events.iter().enumerate() {
        let Some(turn_id) = event.metadata["provider_turn_id"].as_str() else {
            continue;
        };
        let turn_id = turn_id.trim();
        if turn_id.is_empty() {
            continue;
        }
        match event.source.as_deref() {
            Some("response_item") => requests_by_turn.entry(turn_id).or_default().push(index),
            Some("event_msg") => mirrors_by_turn.entry(turn_id).or_default().push(index),
            _ => {}
        }
    }

    let candidate_merges = {
        let unique_owner = |event: &AgentChatEvent| {
            if event.id.trim().is_empty() {
                return None;
            }
            let owners = event_owners.get(event.id.as_str())?;
            (owners.len() == 1).then_some(owners[0])
        };
        let mut candidates = Vec::new();
        for (turn_id, request_indices) in &requests_by_turn {
            let Some(mirror_indices) = mirrors_by_turn.get(turn_id) else {
                continue;
            };
            for &request_index in request_indices {
                let request = &events[request_index];
                let mut matching_mirrors = mirror_indices.iter().copied().filter(|&index| {
                    crate::providers::chat_transcript::codex_user_mirror_pair(
                        request,
                        &events[index],
                    )
                });
                let Some(mirror_index) = matching_mirrors.next() else {
                    continue;
                };
                if matching_mirrors.next().is_some() {
                    continue;
                }
                let mirror = &events[mirror_index];
                let reverse_matches = request_indices
                    .iter()
                    .filter(|&&candidate_index| {
                        crate::providers::chat_transcript::codex_user_mirror_pair(
                            &events[candidate_index],
                            mirror,
                        )
                    })
                    .count();
                if reverse_matches != 1 {
                    continue;
                }

                let (Some(request_owner), Some(mirror_owner)) =
                    (unique_owner(request), unique_owner(mirror))
                else {
                    continue;
                };
                if request_owner == mirror_owner
                    || !is_codex_request_record_for(&records[request_owner], request)
                    || !is_codex_request_record_for(&records[mirror_owner], mirror)
                {
                    continue;
                }
                candidates.push((request_owner, mirror_owner, mirror_index));
            }
        }
        candidates
    };

    // A narrative that appears to own more than one native pair is ambiguous;
    // leave every involved row intact instead of joining turns through text.
    let mut owner_pair_counts = HashMap::<usize, usize>::new();
    for &(request_owner, mirror_owner, _) in &candidate_merges {
        *owner_pair_counts.entry(request_owner).or_default() += 1;
        *owner_pair_counts.entry(mirror_owner).or_default() += 1;
    }
    let merges = candidate_merges
        .into_iter()
        .filter(|(request_owner, mirror_owner, _)| {
            owner_pair_counts.get(request_owner) == Some(&1)
                && owner_pair_counts.get(mirror_owner) == Some(&1)
        })
        .collect::<Vec<_>>();

    let mut removed = Vec::with_capacity(merges.len());
    for (request_owner, mirror_owner, mirror_event_index) in merges {
        let duplicate = records[mirror_owner].clone();
        mark_coalesced_narrative_sequence(&mut events[mirror_event_index], duplicate.seq)?;
        let canonical = &mut records[request_owner];
        if canonical.text.is_none() && duplicate.text.is_some() {
            canonical.text.clone_from(&duplicate.text);
            canonical.excerpt.clone_from(&duplicate.excerpt);
        }
        if canonical.turn_id.is_none() {
            canonical.turn_id.clone_from(&duplicate.turn_id);
        }
        for (target, values) in [
            (&mut canonical.event_refs, duplicate.event_refs),
            (&mut canonical.source_refs, duplicate.source_refs),
            (&mut canonical.artifact_refs, duplicate.artifact_refs),
        ] {
            for value in values {
                if !target.contains(&value) {
                    target.push(value);
                }
            }
        }
        removed.push(mirror_owner);
    }

    if removed.is_empty() {
        return Ok(0);
    }

    removed.sort_unstable();
    removed.dedup();
    let removed_count = removed.len() as u64;
    for index in removed.into_iter().rev() {
        records.remove(index);
    }
    Ok(removed_count)
}

const COALESCED_NARRATIVE_SEQUENCES: &str = "wardian_archive_coalesced_narrative_sequences";

fn mark_coalesced_narrative_sequence(event: &mut AgentChatEvent, sequence: u64) -> io::Result<()> {
    let metadata = event.metadata.as_object_mut().ok_or_else(|| {
        recoverable_recovery_error(format!(
            "Codex mirror event {} has malformed metadata",
            event.id
        ))
    })?;
    let markers = metadata
        .entry(COALESCED_NARRATIVE_SEQUENCES)
        .or_insert_with(|| serde_json::json!([]));
    let markers = markers.as_array_mut().ok_or_else(|| {
        recoverable_recovery_error(format!(
            "Codex mirror event {} has malformed coalesced sequence markers",
            event.id
        ))
    })?;
    if !markers
        .iter()
        .any(|marker| marker.as_u64() == Some(sequence))
    {
        markers.push(serde_json::json!(sequence));
    }
    Ok(())
}

fn verified_coalesced_narrative_sequences(
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
) -> io::Result<HashSet<u64>> {
    let mut retired = super::mock_mirrors::retired_sequences(records, events)?;
    for mirror in events {
        let Some(markers) = mirror.metadata.get(COALESCED_NARRATIVE_SEQUENCES) else {
            continue;
        };
        let invalid = || {
            recoverable_recovery_error(format!(
                "coalesced sequence marker on event {} is not backed by one canonical Codex request pair",
                mirror.id
            ))
        };
        let Some(markers) = markers.as_array().filter(|markers| !markers.is_empty()) else {
            return Err(invalid());
        };
        if !mirror.provider.eq_ignore_ascii_case("codex")
            || mirror.metadata["provider_log"] != true
            || mirror.kind != AgentChatEventKind::Message
            || mirror.role != Some(AgentChatRole::User)
            || mirror.source.as_deref() != Some("event_msg")
        {
            return Err(invalid());
        }
        let mut matching_requests = events.iter().filter(|request| {
            request.source.as_deref() == Some("response_item")
                && crate::providers::chat_transcript::codex_user_mirror_pair(request, mirror)
        });
        let Some(request) = matching_requests.next() else {
            return Err(invalid());
        };
        if matching_requests.next().is_some()
            || events
                .iter()
                .filter(|candidate| {
                    candidate.source.as_deref() == Some("event_msg")
                        && crate::providers::chat_transcript::codex_user_mirror_pair(
                            request, candidate,
                        )
                })
                .count()
                != 1
        {
            return Err(invalid());
        }
        let request_owners = records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| record.event_refs.contains(&request.id).then_some(index))
            .collect::<Vec<_>>();
        let mirror_owners = records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| record.event_refs.contains(&mirror.id).then_some(index))
            .collect::<Vec<_>>();
        if request_owners.len() != 1
            || mirror_owners.as_slice() != request_owners.as_slice()
            || !is_codex_request_record_for(&records[request_owners[0]], request)
        {
            return Err(invalid());
        }
        for marker in markers {
            let Some(sequence) = marker.as_u64().filter(|sequence| *sequence > 0) else {
                return Err(invalid());
            };
            if !retired.insert(sequence) {
                return Err(invalid());
            }
        }
    }
    if records.iter().any(|record| retired.contains(&record.seq)) {
        return Err(recoverable_recovery_error(
            "coalesced narrative sequence is still occupied by a record".to_string(),
        ));
    }
    Ok(retired)
}

fn is_codex_request_record_for(
    record: &ConversationNarrativeRecord,
    event: &AgentChatEvent,
) -> bool {
    let Some(request_root_id) = event.metadata["request_root_id"].as_str() else {
        return false;
    };
    record.kind == ConversationRecordKind::Message
        && record.role.as_deref() == Some("user")
        && record.input_origin == Some(ConversationInputOrigin::HumanInput)
        && record.input_purpose.as_deref() == Some("request")
        && record.request_root_id.as_deref() == Some(request_root_id)
        && event
            .text
            .as_deref()
            .zip(record.text.as_deref())
            .is_none_or(|(event_text, record_text)| event_text == record_text)
}

pub(super) fn matching_record_index(
    records: &[ConversationNarrativeRecord],
    current: &AgentChatEvent,
    archived_events: &[AgentChatEvent],
) -> io::Result<Option<usize>> {
    matching_record_index_with_candidates(records, current, archived_events, &[])
}

pub(super) fn matching_record_index_with_candidates(
    records: &[ConversationNarrativeRecord],
    current: &AgentChatEvent,
    archived_events: &[AgentChatEvent],
    batch_candidates: &[AgentChatEvent],
) -> io::Result<Option<usize>> {
    if !current.metadata["archive_adoption"].is_null() {
        return unique_match(
            records.iter().enumerate().filter_map(|(index, record)| {
                record.event_refs.contains(&current.id).then_some(index)
            }),
            "archive adoption narrative ownership",
        );
    }
    if current.metadata["provider_log"] == true
        && current.provider.eq_ignore_ascii_case("codex")
        && current.kind == AgentChatEventKind::Message
        && current.role == Some(AgentChatRole::Assistant)
    {
        let canonical_event = matching_event_index(archived_events, current)?
            .and_then(|index| archived_events.get(index));
        let exact_match = unique_match(
            records.iter().enumerate().filter_map(|(index, record)| {
                (record.event_refs.contains(&current.id)
                    || canonical_event.is_some_and(|event| record.event_refs.contains(&event.id)))
                .then_some(index)
            }),
            &format!("narrative ownership for event {}", current.id),
        )?;
        if exact_match.is_some() {
            return Ok(exact_match);
        }

        return unique_match(
            records
                .iter()
                .enumerate()
                .filter_map(|(record_index, record)| {
                    archived_events
                        .iter()
                        .any(|archived| {
                            record.event_refs.contains(&archived.id)
                                && super::codex_archive_mirror_pair(
                                    current,
                                    archived,
                                    archived_events
                                        .iter()
                                        .chain(batch_candidates.iter())
                                        .chain(std::iter::once(current)),
                                )
                        })
                        .then_some(record_index)
                }),
            &format!(
                "Codex completion narrative ownership for event {}",
                current.id
            ),
        );
    }

    let is_codex_user_request = current.provider.eq_ignore_ascii_case("codex")
        && current.kind == AgentChatEventKind::Message
        && current.role == Some(AgentChatRole::User)
        && current.source.as_deref() == Some("response_item")
        && current.metadata["raw_type"] == "message"
        && current.metadata["input_origin"] == "human_input"
        && current.metadata["input_purpose"] == "request";
    let is_codex_user_mirror = provenance::is_codex_user_message_mirror(current);
    if current.metadata["provider_log"] == true && (is_codex_user_mirror || is_codex_user_request) {
        let exact_match = unique_match(
            records.iter().enumerate().filter_map(|(index, record)| {
                record.event_refs.contains(&current.id).then_some(index)
            }),
            &format!("narrative ownership for event {}", current.id),
        )?;
        if exact_match.is_some() {
            return Ok(exact_match);
        }

        let paired_observation = unique_match(
            archived_events
                .iter()
                .enumerate()
                .filter_map(|(index, archived)| {
                    crate::providers::chat_transcript::codex_user_mirror_pair(current, archived)
                        .then_some(index)
                }),
            &format!("Codex request mirror for event {}", current.id),
        )?;
        if let Some(archived_event_index) = paired_observation {
            let archived_event = &archived_events[archived_event_index];
            return unique_match(
                records.iter().enumerate().filter_map(|(index, record)| {
                    record
                        .event_refs
                        .iter()
                        .any(|event_ref| event_ref == &archived_event.id)
                        .then_some(index)
                }),
                &format!("Codex mirror narrative ownership for event {}", current.id),
            );
        }

        if !is_codex_user_mirror {
            return Ok(None);
        }

        // A historical text-derived mirror ID may be present as an alias in
        // an archive. Resolve its archived event first so native turn evidence
        // can prevent the alias from attaching a different turn's narrative.
        let Some(archived_event_index) = matching_event_index(archived_events, current)? else {
            return Ok(None);
        };
        let archived_identity_ids = event_identity_ids(&archived_events[archived_event_index]);
        let Some(request_root_id) = current
            .metadata
            .get("request_root_id")
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(None);
        };
        return unique_match(
            records.iter().enumerate().filter_map(|(index, record)| {
                (record.request_root_id.as_deref() == Some(request_root_id)
                    && record
                        .event_refs
                        .iter()
                        .any(|event_ref| archived_identity_ids.contains(&event_ref.as_str())))
                .then_some(index)
            }),
            &format!("legacy narrative ownership for event {}", current.id),
        );
    }

    if current.provider != "claude" || current.metadata["provider_log"] != true {
        let ids = event_identity_ids(current);
        return unique_match(
            records.iter().enumerate().filter_map(|(index, record)| {
                record
                    .event_refs
                    .iter()
                    .any(|event_ref| ids.contains(&event_ref.as_str()))
                    .then_some(index)
            }),
            &format!("narrative ownership for event {}", current.id),
        );
    }

    let exact = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| record.event_refs.contains(&current.id).then_some(index));
    let exact_match = unique_match(
        exact,
        &format!("narrative ownership for event {}", current.id),
    )?;
    if exact_match.is_some() {
        return Ok(exact_match);
    }

    let aliases = event_identity_ids(current);
    unique_match(
        records.iter().enumerate().filter_map(|(index, record)| {
            record
                .event_refs
                .iter()
                .any(|event_ref| aliases.contains(&event_ref.as_str()))
                .then_some(index)
        }),
        &format!("legacy narrative ownership for event {}", current.id),
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
    !event.metadata["chat_source_ref"].is_string()
        && event.metadata["provider_log"] == true
        && event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && record
            .event_refs
            .iter()
            .any(|event_ref| event_ref.starts_with("generated:"))
}

const MAX_RECOVERY_OWNER_REFERENCES: usize = 64 * 1024;

/// Retain original record positions only for this immutable recovery scan.
/// Ordinary provenance keeps its scanner; oversized adoption indexes fall back
/// without truncating owners or introducing errors for unrelated ambiguity.
struct RecoveryRecordOwners<'a> {
    records: &'a [ConversationNarrativeRecord],
    // Outer None is lazy; inner None remembers the complete scanner fallback.
    adoption: Option<Option<HashMap<&'a str, Vec<usize>>>>,
}

impl<'a> RecoveryRecordOwners<'a> {
    fn new(records: &'a [ConversationNarrativeRecord]) -> Self {
        Self {
            records,
            adoption: None,
        }
    }

    fn build_adoption(
        records: &'a [ConversationNarrativeRecord],
    ) -> Option<HashMap<&'a str, Vec<usize>>> {
        let references = records.iter().try_fold(0usize, |count, record| {
            count.checked_add(record.event_refs.len())
        })?;
        if references > MAX_RECOVERY_OWNER_REFERENCES {
            return None;
        }
        let mut owners = HashMap::<&str, Vec<usize>>::new();
        for (index, record) in records.iter().enumerate() {
            for event_ref in &record.event_refs {
                let positions = owners.entry(event_ref.as_str()).or_default();
                // A slice scan yields a record once even if it repeats a ref.
                // Records are visited in order, so its last owner is enough.
                if positions.last() != Some(&index) {
                    positions.push(index);
                }
            }
        }
        Some(owners)
    }

    fn matching_record_index(
        &mut self,
        current: &AgentChatEvent,
        archived_events: &[AgentChatEvent],
    ) -> io::Result<Option<usize>> {
        if current.metadata["archive_adoption"].is_null() {
            return matching_record_index(self.records, current, archived_events);
        }
        let records = self.records;
        match self
            .adoption
            .get_or_insert_with(|| Self::build_adoption(records))
        {
            Some(owners) => unique_match(
                owners
                    .get(current.id.as_str())
                    .into_iter()
                    .flatten()
                    .copied(),
                "archive adoption narrative ownership",
            ),
            None => matching_record_index(records, current, archived_events),
        }
    }
}

pub(super) fn recover_unlinked_observations(
    conversation_id: &str,
    directory: &Path,
    context: &ConversationArchiveContext,
    records: &mut Vec<ConversationNarrativeRecord>,
    events: &[AgentChatEvent],
    recover_only_ids: Option<&HashSet<String>>,
    next_seq: &mut u64,
) -> io::Result<Vec<RecoveredObservation>> {
    super::mock_mirrors::recover_pending(context, directory, records, events)?;
    let mut occupied_sequences = HashSet::new();
    for record in records.iter() {
        if !occupied_sequences.insert(record.seq) {
            return Err(recoverable_recovery_error(format!(
                "duplicate narrative sequence {} prevents recovery",
                record.seq
            )));
        }
    }
    let retired_sequences = verified_coalesced_narrative_sequences(records, events)?;
    occupied_sequences.extend(retired_sequences.iter().copied());
    if let Some(last_reserved_sequence) = records
        .iter()
        .map(|record| record.seq)
        .chain(retired_sequences.iter().copied())
        .max()
    {
        let next_available_sequence = last_reserved_sequence.checked_add(1).ok_or_else(|| {
            recoverable_recovery_error("narrative sequence space is exhausted".to_string())
        })?;
        *next_seq = (*next_seq).max(next_available_sequence);
    }
    let mut recovery_seq = first_missing_sequence(&occupied_sequences)?;
    let mut known_sources: Option<Vec<ConversationSourceRecord>> = None;
    let mut recovered_event_ids = HashSet::new();
    let mut recovered = Vec::new();
    let mut record_owners = RecoveryRecordOwners::new(records);

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
        if record_owners
            .matching_record_index(event, events)?
            .is_some()
        {
            continue;
        }
        let paired_codex_mirror_index = unique_match(
            events.iter().enumerate().filter_map(|(index, candidate)| {
                crate::providers::chat_transcript::codex_user_mirror_pair(event, candidate)
                    .then_some(index)
            }),
            &format!("Codex recovery mirror for event {}", event.id),
        )?;
        if paired_codex_mirror_index.is_some() && event.source.as_deref() != Some("response_item") {
            // Recover the identified request row once, with its exact event_msg
            // observation attached below. The mirror alone cannot establish text.
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

        let mut paired_source = None;
        if let Some(mirror) = paired_codex_mirror_index.map(|index| &events[index]) {
            if !record
                .event_refs
                .iter()
                .any(|event_ref| event_ref == &mirror.id)
            {
                record.event_refs.push(mirror.id.clone());
            }
            paired_source = source_record_from_chat_event(mirror, record.seq);
            if let Some(source) = &paired_source {
                if !record
                    .source_refs
                    .iter()
                    .any(|source_ref| source_ref == &source.source_id)
                {
                    record.source_refs.push(source.source_id.clone());
                }
            }
        }

        if is_generated {
            validate_reconstructable_record(event, &record)?;
        }
        if !occupied_sequences.insert(record.seq) {
            return Err(recoverable_recovery_error(format!(
                "durable observation {} collides with an existing narrative sequence {}",
                event.id, record.seq
            )));
        }

        let mut sources = if is_generated {
            generated_sources_from_record(context, &mut record)
        } else {
            source_record_from_chat_event(event, record.seq)
                .into_iter()
                .collect::<Vec<_>>()
        };
        if let Some(source) = paired_source {
            if !sources
                .iter()
                .any(|existing| existing.source_id == source.source_id)
            {
                sources.push(source);
            }
        }
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
            let known = cached_source_rows(&mut known_sources, &directory.join("sources.jsonl"))?;
            if !source_row_is_published(known, source)? {
                known.push(source.clone());
            }
        }

        recovered_event_ids.extend(event_ids.into_iter().map(ToString::to_string));
        recovery_seq = first_missing_sequence(&occupied_sequences)?;
        *next_seq = (*next_seq).max(record.seq.saturating_add(1));
        recovered.push(RecoveredObservation { record, sources });
    }
    // Publication may sort/replace records. Never retain positions beyond this
    // scan or let staged recovery observations become existing owners early.
    drop(record_owners);

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
        all_sequences.extend(retired_sequences);
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
    events: &[AgentChatEvent],
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
    let retired_sequences = verified_coalesced_narrative_sequences(&repaired_records, events)?;
    let mut all_sequences = repaired_records
        .iter()
        .map(|record| record.seq)
        .collect::<Vec<_>>();
    all_sequences.extend(retired_sequences);
    validate_sequence_prefix(all_sequences)?;

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
    rebuild_derived_projections_inner(agent_id, directory, context, handle, records, events, true)
}

/// Receipt maintenance updates derived summaries without rewriting evidence.
pub(super) fn rebuild_receipt_summaries(
    agent_id: &str,
    directory: &Path,
    context: &ConversationArchiveContext,
    handle: &ActiveConversationHandle,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
) -> io::Result<()> {
    rebuild_derived_projections_inner(agent_id, directory, context, handle, records, events, false)
}

fn rebuild_derived_projections_inner(
    agent_id: &str,
    directory: &Path,
    context: &ConversationArchiveContext,
    handle: &ActiveConversationHandle,
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
    reconcile: bool,
) -> io::Result<()> {
    let Some(first_record) = records.first() else {
        return Ok(());
    };
    let Some(last_record) = records.last() else {
        return Ok(());
    };
    let mut all_events = events.to_vec();
    if reconcile && provenance::bind_delivered_inputs(&mut all_events, records)? {
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

/// Reads `sources.jsonl` only the first time an append needs it. Passing the
/// read to `Option::get_or_insert` would evaluate it on every call and discard
/// the result, re-parsing the whole file once per captured event.
pub(super) fn cached_source_rows<'a>(
    cached: &'a mut Option<Vec<ConversationSourceRecord>>,
    path: &Path,
) -> io::Result<&'a mut Vec<ConversationSourceRecord>> {
    if cached.is_none() {
        *cached = Some(read_jsonl_records(path)?);
    }
    Ok(cached.get_or_insert_with(Vec::new))
}

pub(super) fn append_source_if_needed(
    path: &Path,
    cached: &mut Option<Vec<ConversationSourceRecord>>,
    source: &ConversationSourceRecord,
    repair_existing: bool,
) -> io::Result<bool> {
    if repair_existing {
        let sources = cached_source_rows(cached, path)?;
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

#[cfg(test)]
mod recovery_record_owners_tests {
    use super::*;
    use serde_json::{json, Value};

    fn event(id: &str, adoption: bool, aliases: Value) -> AgentChatEvent {
        serde_json::from_value(json!({
            "id": id,
            "session_id": "agent",
            "provider": "codex",
            "kind": "message",
            "role": "user",
            "text": "original observation",
            "metadata": {
                "provider_log": true,
                "archive_adoption": if adoption { json!({}) } else { Value::Null },
                "legacy_event_ids": aliases
            }
        }))
        .unwrap()
    }

    fn record(seq: u64, refs: &[&str]) -> ConversationNarrativeRecord {
        let mut record = narrative_from_chat_event(&event("seed", false, json!([])), seq).unwrap();
        record.event_refs = refs.iter().map(|id| (*id).to_owned()).collect();
        record.source_refs = vec![format!("src_{seq}")];
        record
    }

    fn outcome(
        result: io::Result<Option<usize>>,
    ) -> Result<Option<usize>, (io::ErrorKind, String)> {
        result.map_err(|error| (error.kind(), error.to_string()))
    }

    #[test]
    fn adoption_owner_index_matches_scanner_and_preserves_original_records() {
        let ambiguous = Err((
            io::ErrorKind::InvalidData,
            "ambiguous archive adoption narrative ownership: multiple durable owners".to_owned(),
        ));
        let cases = vec![
            (
                "missing",
                vec![record(91, &["other"])],
                event("wanted", true, json!([])),
                Ok(None),
            ),
            (
                "original position, nonordinal seq",
                vec![record(91, &["other"]), record(7, &["wanted"])],
                event("wanted", true, json!([])),
                Ok(Some(1)),
            ),
            (
                "reordered records",
                vec![record(7, &["wanted"]), record(91, &["other"])],
                event("wanted", true, json!([])),
                Ok(Some(0)),
            ),
            (
                "repeated ref in one record",
                vec![record(7, &["wanted", "other", "wanted"])],
                event("wanted", true, json!([])),
                Ok(Some(0)),
            ),
            (
                "duplicate owners",
                vec![record(91, &["wanted"]), record(7, &["wanted"])],
                event("wanted", true, json!([])),
                ambiguous,
            ),
            (
                "unrelated ambiguity",
                vec![
                    record(91, &["shared"]),
                    record(7, &["shared"]),
                    record(20, &["wanted"]),
                ],
                event("wanted", true, json!([])),
                Ok(Some(2)),
            ),
            (
                "opaque numeric string",
                vec![
                    record(91, &["01"]),
                    record(7, &["1"]),
                    record(20, &["src_1"]),
                ],
                event("1", true, json!([])),
                Ok(Some(1)),
            ),
            (
                "opaque leading zero",
                vec![record(91, &["1"]), record(7, &["01"])],
                event("01", true, json!([])),
                Ok(Some(1)),
            ),
            (
                "opaque source ref",
                vec![record(91, &["1"]), record(7, &["src_1"])],
                event("src_1", true, json!([])),
                Ok(Some(1)),
            ),
            (
                "string alias is not adoption ownership",
                vec![record(7, &["alias"])],
                event("wanted", true, json!(["alias"])),
                Ok(None),
            ),
            (
                "exact ownership wins over string alias",
                vec![record(91, &["alias"]), record(7, &["wanted"])],
                event("wanted", true, json!(["alias"])),
                Ok(Some(1)),
            ),
            (
                "numeric JSON alias is ignored",
                vec![record(7, &["1"])],
                event("wanted", true, json!([1])),
                Ok(None),
            ),
            (
                "numeric-looking string alias is ignored",
                vec![record(7, &["1"])],
                event("wanted", true, json!(["1"])),
                Ok(None),
            ),
        ];
        for (name, records, current, expected) in cases {
            let before = serde_json::to_value(&records).unwrap();
            assert_eq!(
                outcome(matching_record_index(&records, &current, &[])),
                expected,
                "{name}"
            );
            let mut owners = RecoveryRecordOwners::new(&records);
            for _ in 0..2 {
                assert_eq!(
                    outcome(owners.matching_record_index(&current, &[])),
                    expected,
                    "{name}"
                );
            }
            assert_eq!(serde_json::to_value(&records).unwrap(), before, "{name}");
        }
    }

    #[test]
    fn adoption_owner_index_is_lazy_and_leaves_ordinary_alias_matching_on_the_scanner() {
        let records = vec![record(91, &["alias"]), record(7, &["1"])];
        let mut owners = RecoveryRecordOwners::new(&records);
        let ordinary = event("ordinary", false, json!(["alias", 1]));
        assert_eq!(
            owners.matching_record_index(&ordinary, &[]).unwrap(),
            Some(0)
        );
        assert_eq!(
            outcome(owners.matching_record_index(&ordinary, &[])),
            outcome(matching_record_index(&records, &ordinary, &[]))
        );
        assert!(owners.adoption.is_none());
        let numeric = event("ordinary", false, json!([1]));
        assert_eq!(owners.matching_record_index(&numeric, &[]).unwrap(), None);
        assert!(owners.adoption.is_none());
        let adoption = event("alias", true, json!([]));
        assert_eq!(
            owners.matching_record_index(&adoption, &[]).unwrap(),
            Some(0)
        );
        assert!(owners.adoption.as_ref().unwrap().is_some());
        assert_eq!(
            owners.matching_record_index(&ordinary, &[]).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn adoption_owner_index_cap_falls_back_without_truncating_ambiguous_owners() {
        let mut records = vec![record(91, &["wanted"]), record(7, &["wanted"])];
        records[0].event_refs = vec!["wanted".to_owned(); MAX_RECOVERY_OWNER_REFERENCES];
        let current = event("wanted", true, json!([]));
        let expected = outcome(matching_record_index(&records, &current, &[]));
        assert!(expected.is_err());
        let mut owners = RecoveryRecordOwners::new(&records);
        for _ in 0..2 {
            assert_eq!(
                outcome(owners.matching_record_index(&current, &[])),
                expected
            );
            assert!(matches!(&owners.adoption, Some(None)));
        }
    }

    #[test]
    fn adoption_owner_index_recovery_keeps_linked_records_and_numeric_source_refs() {
        let directory = tempfile::tempdir().unwrap();
        let context = ConversationArchiveContext::for_agent_id("agent", "codex");
        let events = vec![
            event("first", true, json!([])),
            event("second", true, json!([])),
        ];
        let mut records = vec![record(91, &["second"]), record(7, &["first", "first"])];
        let before = serde_json::to_value(&records).unwrap();
        let durable = HashSet::from(["first".to_owned(), "second".to_owned()]);
        let mut next_seq = 1;
        for _ in 0..2 {
            let recovered = recover_unlinked_observations(
                "conversation",
                directory.path(),
                &context,
                &mut records,
                &events,
                Some(&durable),
                &mut next_seq,
            )
            .unwrap();
            assert!(recovered.is_empty());
            assert_eq!(next_seq, 92);
            assert_eq!(serde_json::to_value(&records).unwrap(), before);
        }
    }

    #[cfg(feature = "adoption-throughput")]
    #[test]
    fn adoption_owner_index_component_measurement() {
        use std::{hint::black_box, time::Instant};

        for rows in [512usize, 4096] {
            let events = (0..rows)
                .map(|index| event(&format!("adoption-{index}"), true, json!([])))
                .collect::<Vec<_>>();
            let records = events
                .iter()
                .rev()
                .enumerate()
                .map(|(index, event)| record((index as u64 + 1) * 7, &[event.id.as_str()]))
                .collect::<Vec<_>>();
            let references = records
                .iter()
                .map(|record| record.event_refs.len())
                .sum::<usize>();
            let scanner_started = Instant::now();
            let mut scanner_checksum = 0usize;
            for (index, current) in events.iter().enumerate() {
                let found = black_box(
                    matching_record_index(
                        black_box(records.as_slice()),
                        black_box(current),
                        black_box(events.as_slice()),
                    )
                    .unwrap(),
                );
                assert_eq!(found, Some(rows - index - 1));
                scanner_checksum = scanner_checksum.wrapping_add(found.unwrap());
            }
            black_box(scanner_checksum);
            let scanner_duration = scanner_started.elapsed();

            let indexed_started = Instant::now();
            let mut owners = RecoveryRecordOwners::new(black_box(records.as_slice()));
            let mut indexed_checksum = 0usize;
            for (index, current) in events.iter().enumerate() {
                let found = black_box(
                    owners
                        .matching_record_index(black_box(current), black_box(events.as_slice()))
                        .unwrap(),
                );
                assert_eq!(found, Some(rows - index - 1));
                indexed_checksum = indexed_checksum.wrapping_add(found.unwrap());
            }
            black_box(indexed_checksum);
            let indexed_duration = indexed_started.elapsed();
            assert!(owners.adoption.as_ref().unwrap().is_some());
            assert_eq!(indexed_checksum, scanner_checksum);
            eprintln!(
                "ADOPTION_OWNER_INDEX_COMPONENT {}",
                json!({
                    "records": records.len(), "E": events.len(), "Q": references,
                    "Q_per_E": references as f64 / events.len() as f64,
                    "scanner_ns": scanner_duration.as_nanos(),
                    "index_build_and_queries_ns": indexed_duration.as_nanos(),
                    "scanner_checksum": scanner_checksum, "indexed_checksum": indexed_checksum
                })
            );
        }
    }
}
