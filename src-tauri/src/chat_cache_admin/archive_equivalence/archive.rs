use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use wardian_core::conversations::{
    ConversationLoggingSetting, ConversationManifest, ConversationNarrativeRecord,
    ConversationSourceRecord, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::AgentChatEvent;

use super::component;
use super::copy::{Fault, Reader};
use super::graph::{Payload, Row};
use crate::state::conversation_archive::chat_read::{header, tool_input_body, Head};

const ARCHIVE_BYTES: usize = 16 * 1024 * 1024;
const LINE_BYTES: usize = 1024 * 1024;
const ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
const PREVIEW_BYTES: usize = 1024;

fn jsonl<T: DeserializeOwned>(reader: &mut Reader, path: &Path) -> Result<Vec<T>, Fault> {
    let bytes = reader.file(path, ARCHIVE_BYTES)?;
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(Fault::Invalid);
    }
    let mut records = Vec::new();
    for line in std::str::from_utf8(&bytes)
        .map_err(|_| Fault::Invalid)?
        .lines()
    {
        reader.attempt()?;
        if line.len() > LINE_BYTES {
            return Err(Fault::Limit);
        }
        if !line.trim().is_empty() {
            records.push(serde_json::from_str(line).map_err(|_| Fault::Invalid)?);
        }
    }
    Ok(records)
}

pub(super) struct Archive {
    directory: PathBuf,
    manifest: ConversationManifest,
    events: BTreeMap<String, Option<(usize, AgentChatEvent)>>,
    links: BTreeMap<String, Option<usize>>,
    records: Vec<ConversationNarrativeRecord>,
    sources: BTreeMap<String, Option<ConversationSourceRecord>>,
}

impl Archive {
    pub(super) fn read(reader: &mut Reader, agent: &Path, head: &Head) -> Result<Self, Fault> {
        let directory = agent.join("conversations").join(&head.conversation_id);
        let manifest: ConversationManifest =
            serde_json::from_slice(&reader.file(&directory.join("manifest.json"), LINE_BYTES)?)
                .map_err(|_| Fault::Invalid)?;
        if manifest.schema != CONVERSATION_SCHEMA
            || manifest.agent_id != head.agent_id
            || manifest.conversation_id != head.conversation_id
            || manifest.provider_source_key != head.source_key
            || manifest.provider.is_empty()
            || manifest.effective_logging != ConversationLoggingSetting::Enabled
            || manifest.format_versions.manifest != 1
            || manifest.format_versions.events != 1
            || manifest.format_versions.conversation != 1
            || manifest.format_versions.sources != 1
        {
            return Err(Fault::Invalid);
        }
        let records: Vec<ConversationNarrativeRecord> =
            jsonl(reader, &directory.join("conversation.jsonl"))?;
        if manifest.record_count != records.len() as u64 {
            return Err(Fault::Invalid);
        }
        let mut sequences = HashSet::new();
        let mut links = BTreeMap::new();
        for (index, record) in records.iter().enumerate() {
            if record.schema != CONVERSATION_SCHEMA
                || record.seq == 0
                || !sequences.insert(record.seq)
            {
                return Err(Fault::Invalid);
            }
            for reference in &record.event_refs {
                reader.attempt()?;
                links
                    .entry(reference.clone())
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some(index));
            }
        }
        let mut events = BTreeMap::new();
        for (index, event) in jsonl::<AgentChatEvent>(reader, &directory.join("events.jsonl"))?
            .into_iter()
            .enumerate()
        {
            events
                .entry(event.id.clone())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((index, event)));
        }
        let mut sources = BTreeMap::new();
        if records.iter().any(|record| !record.source_refs.is_empty()) {
            for source in
                jsonl::<ConversationSourceRecord>(reader, &directory.join("sources.jsonl"))?
            {
                sources
                    .entry(source.source_id.clone())
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some(source));
            }
        }
        Ok(Self {
            directory,
            manifest,
            events,
            links,
            records,
            sources,
        })
    }

    pub(super) fn unpaired(&self, projected_ids: &HashSet<String>) -> usize {
        self.events
            .keys()
            .filter(|id| !projected_ids.contains(*id))
            .count()
    }

    pub(super) fn event(
        &self,
        id: &str,
    ) -> Result<(&AgentChatEvent, &ConversationNarrativeRecord, usize), Fault> {
        let (position, event) = self
            .events
            .get(id)
            .ok_or(Fault::Missing)?
            .as_ref()
            .ok_or(Fault::Invalid)?;
        let record = self
            .links
            .get(id)
            .copied()
            .flatten()
            .ok_or(Fault::Invalid)?;
        let record = &self.records[record];
        if event.session_id != self.manifest.agent_id
            || event.provider != self.manifest.provider
            || event.metadata["chat_provisional"] == true
            || event.id.starts_with("preview:")
            || !optional_scope(&event.metadata["conversation_archive_id"], |scope| {
                scope == self.manifest.conversation_id
            })
            || !optional_scope(&event.metadata["provider_session_id"], |session| {
                self.manifest
                    .provider_session_ids
                    .iter()
                    .any(|id| id == session)
            })
            || !record.source_refs.iter().all(|reference| {
                self.sources
                    .get(reference)
                    .and_then(Option::as_ref)
                    .is_some_and(|source| {
                        source.schema == CONVERSATION_SCHEMA
                            && source.provider == self.manifest.provider
                            && source.provider_session_id.as_ref().is_none_or(|session| {
                                self.manifest.provider_session_ids.contains(session)
                            })
                    })
            })
        {
            return Err(Fault::Invalid);
        }
        Ok((event, record, *position))
    }

    pub(super) fn payload(
        &self,
        reader: &mut Reader,
        event: &AgentChatEvent,
        record: &ConversationNarrativeRecord,
    ) -> Result<Option<Payload>, Fault> {
        let input = tool_input_body(event);
        let refs = &event.metadata["text_artifact_refs"];
        let refs = if refs.is_null() {
            &event.metadata["archive_record"]["artifact_refs"]
        } else {
            refs
        };
        let artifact = match refs {
            Value::Null => None,
            value => match value.as_array().ok_or(Fault::Invalid)?.as_slice() {
                [] => None,
                [reference] => Some(artifact_path(reference.as_str().ok_or(Fault::Invalid)?)?),
                _ => return Err(Fault::Invalid),
            },
        };
        let excerpt_only = !(event.metadata["text_excerpt"].is_null()
            || event.metadata["text_excerpt"] == false)
            || (event.text.is_none() && record.text.is_none() && record.excerpt.is_some());
        let text = event.text.as_deref().filter(|text| !text.is_empty());
        let (output, inline_body) =
            match (text, artifact) {
                (Some(text), _) if text.len() > PREVIEW_BYTES && !excerpt_only => {
                    (text.as_bytes().to_vec(), true)
                }
                (_, Some(artifact)) => {
                    if !record.artifact_refs.iter().any(|reference| {
                        artifact_path(reference).is_ok_and(|path| path == artifact)
                    }) {
                        return Err(Fault::Invalid);
                    }
                    let bytes = reader.file(&self.directory.join(artifact), ARTIFACT_BYTES)?;
                    if std::str::from_utf8(&bytes).is_err() {
                        return Err(Fault::Invalid);
                    }
                    (bytes, true)
                }
                (_, None) if excerpt_only || !record.artifact_refs.is_empty() => {
                    return Err(Fault::Invalid)
                }
                (Some(text), None) if input.is_some() => (text.as_bytes().to_vec(), true),
                _ => (Vec::new(), false),
            };
        if input.is_none() && !inline_body {
            return Ok(None);
        }
        let mut hash = Sha256::new();
        let mut bytes = output.len() as u64;
        if let Some(input) = input {
            hash.update(input.as_bytes());
            bytes += input.len() as u64;
            if inline_body {
                hash.update(b"\n\n");
                bytes += 2;
            }
        }
        hash.update(&output);
        Ok(Some(Payload {
            bytes,
            digest: format!("{:x}", hash.finalize()),
        }))
    }
}

fn optional_scope(value: &Value, contains: impl FnOnce(&str) -> bool) -> bool {
    value.is_null() || value.as_str().is_some_and(contains)
}

fn artifact_path(reference: &str) -> Result<PathBuf, Fault> {
    let parts = reference.split(['/', '\\']).collect::<Vec<_>>();
    if parts.len() > 8 || !parts.iter().all(|part| component(part)) {
        return Err(Fault::Unsafe);
    }
    match parts.as_slice() {
        [name] => Ok(Path::new("artifacts").join(name)),
        ["artifacts", _, ..] => Ok(parts.into_iter().collect()),
        _ => Err(Fault::Unsafe),
    }
}

pub(super) fn header_matches(row: &Row, event: &AgentChatEvent) -> bool {
    let mut expected = header(event);
    expected.sequence = row.event.sequence;
    let mut actual = row.event.clone();
    // These fields are projection-only. Token, turn and alias equivalence are
    // outside the measured result even when byte equivalence is complete.
    if let Some(metadata) = actual.metadata.as_object_mut() {
        for name in [
            "chat_turn_key",
            "chat_turn_item_key",
            "chat_step_output_start",
            "chat_body_unavailable",
            "chat_body_pending",
        ] {
            metadata.remove(name);
        }
    }
    actual == expected && row.relation.is_none() && row.removed_display_ids.is_empty()
}
