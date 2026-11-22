//! A vacancy's immutable physical neighbours during one serialized publication.
use super::super::event_identity_ids;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    hash::Hash,
    io,
    path::Path,
};
use wardian_core::archive_vacancy::{validate_vacancies, VacancyConflict};
use wardian_core::conversations::{
    write_jsonl_atomic, ConversationNarrativeRecord, ConversationSourceRecord,
};
use wardian_core::models::chat::AgentChatEvent;

type Physical<K, T> = HashMap<K, (T, Vec<u8>)>;

/// Construct under the agent archive lock, before any body or canonical write.
/// This holds the already validated prefix while normal publication owns its tail.
pub(crate) struct Publication {
    sequences: BTreeSet<u64>,
    records: Physical<u64, ConversationNarrativeRecord>,
    events: Physical<String, AgentChatEvent>,
}

impl Publication {
    pub(crate) fn load(directory: &Path, conversation_id: &str) -> io::Result<Self> {
        let sequences = validate_vacancies(directory, conversation_id)?;
        let mut result = Self {
            sequences,
            records: HashMap::new(),
            events: HashMap::new(),
        };
        if result.sequences.is_empty() {
            return Ok(result);
        }
        let protected = result
            .sequences
            .iter()
            .flat_map(|n| [n - 1, n + 1])
            .collect::<HashSet<_>>();
        let records = physical_rows::<_, ConversationNarrativeRecord>(
            &directory.join("conversation.jsonl"),
            |r| r.seq,
        )?;
        result.records = records
            .into_iter()
            .filter(|(n, _)| protected.contains(n))
            .collect();
        let references = result
            .records
            .values()
            .flat_map(|(r, _)| r.event_refs.iter().cloned())
            .collect::<HashSet<_>>();
        result.events =
            physical_rows::<_, AgentChatEvent>(&directory.join("events.jsonl"), |e| e.id.clone())?
                .into_iter()
                .filter(|(_, (e, _))| {
                    event_identity_ids(e)
                        .iter()
                        .any(|id| references.contains(*id))
                })
                .collect();
        Ok(result)
    }

    pub(crate) fn sequences(&self) -> impl Iterator<Item = u64> + '_ {
        self.sequences.iter().copied()
    }
    pub(crate) fn has_vacancies(&self) -> bool {
        !self.sequences.is_empty()
    }
    pub(crate) fn protects(&self, sequence: u64) -> bool {
        self.records.contains_key(&sequence)
    }
    pub(crate) fn conflict(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            VacancyConflict {
                sequence: self.sequences.first().copied().unwrap_or(0),
                rule: "neighbour_hashes_match".into(),
            },
        )
    }

    pub(crate) fn conflict_for(&self, record_sequence: u64) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            VacancyConflict {
                sequence: self
                    .sequences
                    .iter()
                    .find(|n| n.abs_diff(record_sequence) == 1)
                    .copied()
                    .unwrap_or(0),
                rule: "neighbour_hashes_match".into(),
            },
        )
    }

    /// A replay may carry the full text whose exact bytes are already archived.
    /// Retain that proven representation; unrelated metadata still must match.
    pub(crate) fn normalize_replays(
        &self,
        directory: &Path,
        events: &mut [AgentChatEvent],
    ) -> io::Result<()> {
        for (record, _) in self
            .records
            .values()
            .filter(|(r, _)| r.text.is_none() && r.artifact_refs.len() == 1)
        {
            for event in events.iter_mut().filter(|event| {
                self.events.contains_key(&event.id)
                    && event_identity_ids(event)
                        .iter()
                        .any(|id| record.event_refs.iter().any(|r| r.as_str() == *id))
            }) {
                if let Some(text) = event.text.as_deref() {
                    // The constructor validated this unchanged owned reference.
                    if wardian_core::archive_vacancy::body_matches_text(
                        directory,
                        &record.artifact_refs[0],
                        text,
                    )? {
                        *event = super::super::event_record_for_jsonl(event, record);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn check_records(&self, records: &[ConversationNarrativeRecord]) -> io::Result<()> {
        check_protected(records, &self.records, |r| r.seq).map_err(|_| {
            let sequence = self
                .records
                .iter()
                .find(|(n, (old, _))| records.iter().find(|r| r.seq == **n) != Some(old))
                .map(|(n, _)| *n);
            sequence.map_or_else(|| self.conflict(), |n| self.conflict_for(n))
        })
    }
    pub(crate) fn check_events(&self, events: &[AgentChatEvent]) -> io::Result<()> {
        check_protected(events, &self.events, |e| e.id.clone()).map_err(|_| self.conflict())
    }

    /// Replaying an unchanged physical owner needs no enrichment or body write.
    /// A new alias or source would change its protected closure and is rejected.
    pub(crate) fn check_retry(
        &self,
        record: &ConversationNarrativeRecord,
        current: &AgentChatEvent,
        directory: &Path,
        archived: Option<&AgentChatEvent>,
        source: Option<&ConversationSourceRecord>,
        known: &[ConversationSourceRecord],
    ) -> io::Result<()> {
        let owned = archived.is_some_and(|event| {
            event_identity_ids(event)
                .iter()
                .any(|id| record.event_refs.iter().any(|r| r.as_str() == *id))
        });
        let source_unchanged = source.is_none_or(|source| {
            record.source_refs.contains(&source.source_id) && known.contains(source)
        });
        let late_turn_update = current.turn_id.is_some()
            && (record.turn_id.is_none() || current.metadata["provider_phase"] == "final_answer")
            && record.turn_id != current.turn_id;
        let mut completion_changes_body = false;
        if current.metadata["provider_phase"] == "final_answer" {
            if let Some(text) = current.text.as_deref() {
                let same = if let Some(stored) = record.text.as_deref() {
                    stored == text
                } else if record.artifact_refs.len() == 1 {
                    wardian_core::archive_vacancy::body_matches_text(
                        directory,
                        &record.artifact_refs[0],
                        text,
                    )?
                } else {
                    false
                };
                completion_changes_body = !same;
            }
        }
        if !owned || !source_unchanged || late_turn_update || completion_changes_body {
            return Err(self.conflict_for(record.seq));
        }
        Ok(())
    }

    /// Keep the original preparation order without a ledger. Protected capture
    /// defers body writes until recovery and all prefix preflights have passed.
    pub(crate) fn materialize_refresh(
        &self,
        directory: &Path,
        before: &[ConversationNarrativeRecord],
        records: &mut [ConversationNarrativeRecord],
        events: &mut [AgentChatEvent],
        native_ids: &HashSet<String>,
    ) -> io::Result<()> {
        self.check_records(records)?;
        self.check_events(events)?;
        let before_by_seq = super::super::index_records_by_sequence(before);
        for record in records.iter_mut() {
            if !super::super::record_was_present(record, &before_by_seq) {
                super::super::materialize_record_text(directory, record)?;
            }
        }
        for event in events {
            if super::super::provenance::completed_native_tool(event)
                || native_ids.contains(&event.id)
            {
                if let Some(record) = records
                    .iter()
                    .find(|record| record.event_refs.contains(&event.id))
                {
                    *event = super::super::event_record_for_jsonl(event, record);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_records(
        &self,
        path: &Path,
        records: &[ConversationNarrativeRecord],
    ) -> io::Result<Option<Vec<u8>>> {
        if self.sequences.is_empty() {
            return Ok(None);
        }
        self.check_records(records)?;
        prepare(path, records, &self.records, |r| r.seq, || self.conflict()).map(Some)
    }
    pub(crate) fn write_records(
        &self,
        path: &Path,
        records: &[ConversationNarrativeRecord],
    ) -> io::Result<()> {
        if let Some(bytes) = self.prepare_records(path, records)? {
            replace(path, &bytes)
        } else {
            write_jsonl_atomic(path, records)
        }
    }
    pub(crate) fn write_events(&self, path: &Path, events: &[AgentChatEvent]) -> io::Result<()> {
        if self.sequences.is_empty() {
            return write_jsonl_atomic(path, events);
        }
        self.check_events(events)?;
        let bytes = prepare(
            path,
            events,
            &self.events,
            |e| e.id.clone(),
            || self.conflict(),
        )?;
        replace(path, &bytes)
    }
}

fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::utils::codex_messaging::atomic_replace(path, bytes).map_err(io::Error::other)
}

fn check_protected<K: Eq + Hash, T: PartialEq>(
    desired: &[T],
    protected: &Physical<K, T>,
    key: impl Fn(&T) -> K,
) -> io::Result<()> {
    if protected.is_empty() {
        return Ok(());
    }
    let rows = desired
        .iter()
        .map(|r| (key(r), r))
        .collect::<HashMap<_, _>>();
    if rows.len() != desired.len()
        || protected
            .iter()
            .any(|(k, (row, _))| rows.get(k).is_none_or(|r| **r != *row))
    {
        return Err(io::Error::other("protected physical row would change"));
    }
    Ok(())
}

fn physical_rows<K: Eq + Hash, T: DeserializeOwned>(
    path: &Path,
    key: impl Fn(&T) -> K,
) -> io::Result<Physical<K, T>> {
    let mut result = HashMap::new();
    for line in fs::read(path)?.split_inclusive(|b| *b == b'\n') {
        let row: T = serde_json::from_slice(line).map_err(io::Error::other)?;
        if result.insert(key(&row), (row, line.to_vec())).is_some() {
            return Err(io::Error::other("duplicate physical identity"));
        }
    }
    Ok(result)
}

/// Retain exact existing rows when their DTO is unchanged; serialize only updates.
/// Protected physical payloads must also remain byte-identical to the lock snapshot.
fn prepare<K: Eq + Hash, T: DeserializeOwned + Serialize + PartialEq>(
    path: &Path,
    desired: &[T],
    protected: &Physical<K, T>,
    key: impl Fn(&T) -> K,
    conflict: impl Fn() -> io::Error,
) -> io::Result<Vec<u8>> {
    let old: Physical<K, T> = physical_rows(path, &key)?;
    for (k, (_, bytes)) in protected {
        if old.get(k).is_none_or(|(_, current)| current != bytes) {
            return Err(conflict());
        }
    }
    let mut bytes = Vec::new();
    for row in desired {
        if let Some((_, raw)) = old.get(&key(row)).filter(|(prior, _)| prior == row) {
            bytes.extend_from_slice(raw);
        } else {
            serde_json::to_writer(&mut bytes, row).map_err(io::Error::other)?;
            bytes.push(b'\n');
        }
    }
    Ok(bytes)
}
