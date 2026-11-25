//! Chat reads from owned, framed archive records. The index retains file
//! positions and binding evidence only. Payloads are opened for each page.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use wardian_core::conversations::{
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationRecordKind, ConversationTurnRecord,
    CONVERSATION_SCHEMA, CONVERSATION_TURNS_SCHEMA,
};
use wardian_core::models::chat::{AgentChatDetail, AgentChatEvent, AgentChatPage};

#[path = "archive_chat_presentation.rs"]
mod presentation;

use super::chat_read::{clip, header, inline_body_text, BODY_BYTES, IPC_BYTES, PREVIEW_BYTES};
use super::storage::digest;
use super::storage::{
    capture_state_path, conversation_dir, index_path, is_reparse_point, open_read_nofollow,
};
use super::ConversationArchiveContext;
use crate::commands::provider_log_acquisition::{
    native_file_identity, ProviderLogDisabledSpan, ProviderLogNativeIdentity,
};

const FRAME_LIMIT: u64 = 16 * 1024 * 1024;
const PAGE_READ_LIMIT: u64 = 2 * FRAME_LIMIT;
const TOKEN: &str = "archive1";
const ARTIFACT_PROBE_LIMIT: usize = 128;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn plain_file(path: &Path) -> io::Result<File> {
    for ancestor in path
        .ancestors()
        .skip(1)
        .filter(|path| !path.as_os_str().is_empty())
    {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || is_reparse_point(&metadata) {
            return Err(invalid("unsafe chat archive directory"));
        }
    }
    open_read_nofollow(path)
}

fn optional_file(path: &Path) -> io::Result<Option<File>> {
    match plain_file(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn hash_json(value: &impl Serialize) -> io::Result<String> {
    Ok(digest(
        &serde_json::to_vec(value).map_err(io::Error::other)?,
    ))
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Stamp {
    identity: ProviderLogNativeIdentity,
    len: u64,
    modified: SystemTime,
}

impl Stamp {
    fn of(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            identity: native_file_identity(file)?,
            len: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

fn path_stamp(path: &Path) -> io::Result<Option<Stamp>> {
    optional_file(path)?.as_ref().map(Stamp::of).transpose()
}

// Deserialize only admission evidence. Capture normalizer state and legacy
// claims never participate in archive display or cursor authority.
#[derive(Deserialize)]
struct SourceAdmission {
    provider_source_key: String,
    path: String,
    native_identity: ProviderLogNativeIdentity,
    committed_offset: u64,
    #[serde(default)]
    status: String,
    unknown_before_offset: Option<u64>,
    #[serde(default)]
    policy_generation: u64,
    #[serde(default)]
    disabled_spans: Vec<ProviderLogDisabledSpan>,
    open_disabled_from: Option<u64>,
}

fn source_admission(
    context: &ConversationArchiveContext,
    path: &Path,
) -> io::Result<(Option<SourceAdmission>, Option<Stamp>)> {
    let Some(key) = context.provider_source_key.as_deref() else {
        return Ok((None, None));
    };
    let Some(mut file) = optional_file(path)? else {
        return Ok((None, None));
    };
    #[derive(Deserialize)]
    struct CaptureAdmission {
        #[serde(default)]
        provider_log_sources: Vec<SourceAdmission>,
    }
    let stamp = Stamp::of(&file)?;
    let capture: CaptureAdmission =
        serde_json::from_slice(&read_bounded(&mut file)?).map_err(io::Error::other)?;
    let mut matches = capture
        .provider_log_sources
        .into_iter()
        .filter(|source| source.provider_source_key == key);
    let admission = matches.next();
    if matches.next().is_some() {
        return Err(invalid("conflicting chat archive capture ownership"));
    }
    if Stamp::of(&file)? != stamp || path_stamp(path)?.as_ref() != Some(&stamp) {
        return Err(invalid(
            "chat archive capture ownership changed during read",
        ));
    }
    Ok((admission, Some(stamp)))
}

#[derive(Debug, Clone)]
struct Frame {
    offset: u64,
    len: u64,
    proof: String,
}

impl Frame {
    fn load(&self, file: &mut File) -> io::Result<Vec<u8>> {
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = vec![0; self.len as usize];
        file.read_exact(&mut bytes)?;
        if digest(&bytes) != self.proof {
            return Err(invalid("chat archive frame changed"));
        }
        Ok(bytes)
    }
}

#[derive(Debug)]
struct Journal<V> {
    stamp: Option<Stamp>,
    frames: Vec<(Frame, V)>,
    through: u64,
}

#[derive(Default)]
struct IndexReads {
    bytes: usize,
    records: usize,
}

impl<V> Default for Journal<V> {
    fn default() -> Self {
        Self {
            stamp: None,
            frames: Vec::new(),
            through: 0,
        }
    }
}

impl<V> Journal<V> {
    /// Resume only after proving the old prefix endpoints. Every returned
    /// frame is independently checked by digest, including on a warm index.
    fn refresh<T: DeserializeOwned>(
        &mut self,
        path: &Path,
        reads: &mut IndexReads,
        position: impl Fn(T) -> io::Result<V>,
    ) -> io::Result<bool> {
        let Some(mut file) = optional_file(path)? else {
            let changed = self.stamp.is_some();
            *self = Self::default();
            return Ok(changed);
        };
        let stamp = Stamp::of(&file)?;
        if self.stamp.as_ref() == Some(&stamp) {
            return Ok(false);
        }
        let append = self
            .stamp
            .as_ref()
            .is_some_and(|old| old.identity == stamp.identity && old.len < stamp.len)
            && self
                .frames
                .first()
                .is_none_or(|(frame, _)| frame.load(&mut file).is_ok())
            && self
                .frames
                .last()
                .is_none_or(|(frame, _)| frame.load(&mut file).is_ok());
        let mut offset = if append { self.through } else { 0 };
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file);
        let mut added = Vec::new();
        let mut bytes = Vec::new();
        while offset < stamp.len {
            bytes.clear();
            let count = (&mut reader)
                .take((stamp.len - offset).min(FRAME_LIMIT + 1))
                .read_until(b'\n', &mut bytes)? as u64;
            reads.bytes += count as usize;
            if count > FRAME_LIMIT {
                return Err(invalid("chat archive frame exceeds read limit"));
            }
            if bytes.last() != Some(&b'\n') {
                break;
            }
            if !bytes.iter().all(u8::is_ascii_whitespace) {
                let item = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                reads.records += 1;
                added.push((
                    Frame {
                        offset,
                        len: count,
                        proof: digest(&bytes),
                    },
                    position(item)?,
                ));
            }
            offset += count;
        }
        if Stamp::of(reader.get_ref())? != stamp
            || optional_file(path)?
                .as_ref()
                .map(Stamp::of)
                .transpose()?
                .as_ref()
                != Some(&stamp)
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "chat archive changed during indexing",
            ));
        }
        if !append {
            self.frames.clear();
        }
        self.frames.extend(added);
        self.stamp = Some(stamp);
        self.through = offset;
        Ok(true)
    }

    fn pending(&self) -> bool {
        self.stamp
            .as_ref()
            .is_some_and(|stamp| self.through < stamp.len)
    }

    fn current(&self, path: &Path) -> io::Result<bool> {
        Ok(optional_file(path)?.as_ref().map(Stamp::of).transpose()? == self.stamp)
    }
}

#[derive(Debug)]
struct Listing {
    id: String,
    agent: String,
    provider: String,
    started: String,
    records: u64,
}

fn listing(entry: ConversationIndexEntry) -> io::Result<Listing> {
    if entry.schema != CONVERSATION_SCHEMA {
        return Err(invalid("unsupported chat archive index"));
    }
    Ok(Listing {
        id: entry.conversation_id,
        agent: entry.agent_id,
        provider: entry.provider,
        started: entry.started_at,
        records: entry.record_count,
    })
}

#[derive(Debug)]
struct NarrativePosition {
    seq: u64,
    refs: Vec<String>,
    artifacts: Vec<String>,
    missing_event: bool,
}

#[derive(Debug)]
struct EventPosition {
    identity: String,
    coalesced: Vec<(u64, String)>,
    body: Result<Option<String>, ()>,
}

#[derive(Debug)]
struct TurnPosition {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone)]
struct RowPosition {
    event: usize,
    narrative: usize,
    seq: u64,
}

#[derive(Debug, Default)]
struct ConversationPositions {
    events: Journal<EventPosition>,
    narrative: Journal<NarrativePosition>,
    turns: Journal<TurnPosition>,
    turn_ranges: Vec<(usize, u64)>,
    artifacts: Vec<String>,
    rows: Vec<RowPosition>,
    incomplete: bool,
}

impl ConversationPositions {
    fn refresh(
        &mut self,
        directory: &Path,
        conversation: &str,
        reads: &mut IndexReads,
    ) -> io::Result<()> {
        let events_changed = self.events.refresh(
            &directory.join("events.jsonl"),
            reads,
            |event: AgentChatEvent| {
                use super::provenance::{CoalescedInput, COALESCED_INPUTS, REFRESHED_OBSERVATIONS};
                let mut coalesced = Vec::new();
                for (field, generated) in
                    [(COALESCED_INPUTS, true), (REFRESHED_OBSERVATIONS, false)]
                {
                    let Some(value) = event.metadata.get(field) else {
                        continue;
                    };
                    let receipts: Vec<CoalescedInput> =
                        serde_json::from_value(value.clone()).map_err(io::Error::other)?;
                    for receipt in receipts {
                        if receipt.version != 1
                            || receipt.removed_id == event.id
                            || (event.metadata["generated"] == true) != generated
                            || event.metadata["provider_log"] != true
                            || (generated
                                && event.id
                                    != format!("generated:{conversation}:{}", receipt.sequence))
                            || !super::event_identity_ids(&event)
                                .contains(&receipt.removed_id.as_str())
                        {
                            return Err(invalid("invalid archive coalescence receipt"));
                        }
                        coalesced.push((receipt.sequence, digest(receipt.removed_id.as_bytes())));
                    }
                }
                Ok(EventPosition {
                    identity: digest(event.id.as_bytes()),
                    coalesced,
                    body: body_reference(&event)
                        .map(|reference| reference.map(str::to_owned))
                        .map_err(|_| ()),
                })
            },
        )?;
        let narrative_changed = self.narrative.refresh(
            &directory.join("conversation.jsonl"),
            reads,
            |record: ConversationNarrativeRecord| {
                if record.schema != CONVERSATION_SCHEMA || record.seq == 0 {
                    return Err(invalid("unsupported chat archive narrative record"));
                }
                Ok(NarrativePosition {
                    seq: record.seq,
                    artifacts: record.artifact_refs.clone(),
                    missing_event: record.event_refs.is_empty()
                        && !matches!(record.kind, ConversationRecordKind::Lifecycle)
                        && (record.text.is_some()
                            || record.excerpt.is_some()
                            || !record.artifact_refs.is_empty()),
                    refs: record
                        .event_refs
                        .iter()
                        .map(|id| digest(id.as_bytes()))
                        .collect(),
                })
            },
        )?;
        let turns_changed = self.turns.refresh(
            &directory.join("turns.jsonl"),
            reads,
            |turn: ConversationTurnRecord| {
                if !(1..=CONVERSATION_TURNS_SCHEMA).contains(&turn.schema)
                    || turn.conversation_id != conversation
                    || turn.seq_start > turn.seq_end
                {
                    return Err(invalid("unowned chat archive turn"));
                }
                Ok(TurnPosition {
                    start: turn.seq_start,
                    end: turn.seq_end,
                })
            },
        )?;
        if turns_changed {
            let mut ordered = (0..self.turns.frames.len()).collect::<Vec<_>>();
            ordered.sort_unstable_by_key(|&index| self.turns.frames[index].1.start);
            let mut prior_end = 0;
            self.turn_ranges = ordered
                .into_iter()
                .map(|index| {
                    prior_end = prior_end.max(self.turns.frames[index].1.end);
                    (index, prior_end)
                })
                .collect();
        }
        if narrative_changed {
            self.artifacts = self
                .narrative
                .frames
                .iter()
                .flat_map(|(_, record)| record.artifacts.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        if events_changed || narrative_changed {
            let mut by_id = HashMap::new();
            let mut ambiguous = HashSet::new();
            let mut sequences = HashSet::new();
            for (index, (_, record)) in self.narrative.frames.iter().enumerate() {
                if !sequences.insert(record.seq) {
                    return Err(invalid("duplicate chat archive narrative sequence"));
                }
                for reference in &record.refs {
                    if by_id.insert(reference, (index, record.seq)).is_some() {
                        ambiguous.insert(reference);
                    }
                }
            }
            let mut seen = HashSet::new();
            for (_, event) in &self.events.frames {
                let reference = &event.identity;
                if !seen.insert(reference) {
                    ambiguous.insert(reference);
                }
            }
            self.rows = self
                .events
                .frames
                .iter()
                .enumerate()
                .filter_map(|(event, (_, position))| {
                    let reference = &position.identity;
                    if ambiguous.contains(reference) {
                        return None;
                    }
                    by_id.get(reference).map(|&(narrative, seq)| RowPosition {
                        event,
                        narrative,
                        seq,
                    })
                })
                .collect();
            self.rows.sort_by_key(|row| (row.seq, row.event));
            let mut covered = HashSet::new();
            let mut conflicting = false;
            for row in &self.rows {
                let position = &self.events.frames[row.event].1;
                covered.insert(&position.identity);
                let record = &self.narrative.frames[row.narrative].1;
                conflicting |= match &position.body {
                    Ok(Some(reference)) => !record.artifacts.contains(reference),
                    Ok(None) => false,
                    Err(()) => true,
                };
                for (seq, alias) in &position.coalesced {
                    if *seq != row.seq
                        || seen.contains(alias)
                        || ambiguous.contains(alias)
                        || by_id.get(alias) != Some(&(row.narrative, row.seq))
                        || !covered.insert(alias)
                    {
                        conflicting = true;
                    }
                }
            }
            self.incomplete = conflicting
                || self.rows.len() != self.events.frames.len()
                || covered.len() != by_id.len()
                || self
                    .narrative
                    .frames
                    .iter()
                    .any(|(_, record)| record.missing_event);
        }
        Ok(())
    }

    fn current(&self, directory: &Path) -> io::Result<bool> {
        Ok(self.events.current(&directory.join("events.jsonl"))?
            && self
                .narrative
                .current(&directory.join("conversation.jsonl"))?
            && self.turns.current(&directory.join("turns.jsonl"))?)
    }

    fn row_proof(&self, row: &RowPosition) -> String {
        digest(
            format!(
                "{}:{}",
                self.events.frames[row.event].0.proof, self.narrative.frames[row.narrative].0.proof
            )
            .as_bytes(),
        )
    }

    fn turn_frame(&self, seq: u64) -> Option<&Frame> {
        let first = self
            .turn_ranges
            .partition_point(|(_, prior_end)| *prior_end < seq);
        let mut turns = self.turn_ranges[first..]
            .iter()
            .map(|(index, _)| &self.turns.frames[*index])
            .take_while(|(_, turn)| turn.start <= seq)
            .filter(|(_, turn)| seq <= turn.end);
        turns
            .next()
            .filter(|_| turns.next().is_none())
            .map(|(frame, _)| frame)
    }
}

#[derive(Debug, Default)]
pub(super) struct Positions {
    index: Journal<Listing>,
    conversations: HashMap<String, ConversationPositions>,
}

pub(crate) struct Request {
    pub(crate) before: Option<String>,
    pub(crate) revision: Option<String>,
    pub(crate) detail: Option<String>,
    pub(crate) limit: usize,
    pub(crate) logging_enabled: bool,
}

pub(super) fn input_fence(
    context: &ConversationArchiveContext,
    conversation: &str,
    path: Option<&Path>,
) -> io::Result<Option<super::chat_read::InputFence>> {
    let directory = conversation_dir(&context.agent_id, conversation)?;
    let listing = Listing {
        id: conversation.into(),
        agent: context.agent_id.clone(),
        provider: context.provider.clone(),
        started: String::new(),
        records: 0,
    };
    let Some(owner) = select_manifest(&directory, &listing, context)? else {
        return Ok(None);
    };
    if owner.manifest.status != wardian_core::conversations::ConversationStatus::Open {
        return Ok(None);
    }
    let mut source_epoch = None;
    let mut policy_generation = None;
    if matches!(context.provider.as_str(), "codex" | "claude" | "pi") {
        let (source, _) = source_admission(context, &capture_state_path(&context.agent_id)?)?;
        let Some(source) = source else {
            return Ok(None);
        };
        if source.open_disabled_from.is_some()
            || native_file_identity(&open_read_nofollow(
                path.unwrap_or_else(|| Path::new(&source.path)),
            )?)? != source.native_identity
        {
            return Ok(None);
        }
        source_epoch = Some(hash_json(&source.native_identity)?);
        policy_generation = Some(source.policy_generation);
    }
    Ok(Some(super::chat_read::InputFence {
        conversation_id: conversation.into(),
        source_epoch,
        policy_generation,
    }))
}

struct Owned {
    manifest: ConversationManifest,
    stamp: Stamp,
    directory: PathBuf,
    binding: String,
    records: u64,
}

fn select_manifest(
    directory: &Path,
    entry: &Listing,
    context: &ConversationArchiveContext,
) -> io::Result<Option<Owned>> {
    let path = directory.join("manifest.json");
    let Some(mut file) = optional_file(&path)? else {
        return Err(invalid("chat archive manifest is missing"));
    };
    let stamp = Stamp::of(&file)?;
    let value = read_bounded(&mut file)?;
    let manifest: ConversationManifest =
        serde_json::from_slice(&value).map_err(io::Error::other)?;
    if manifest.schema != CONVERSATION_SCHEMA
        || manifest.agent_id != context.agent_id
        || manifest.conversation_id != entry.id
        || manifest.provider != context.provider
    {
        return Err(invalid("chat archive manifest ownership mismatch"));
    }
    let same_sessions = manifest
        .provider_session_ids
        .iter()
        .all(|id| context.provider_session_ids.contains(id))
        && context
            .provider_session_ids
            .iter()
            .all(|id| manifest.provider_session_ids.contains(id));
    let corroborated_legacy =
        manifest.provider_source_key.is_none() && !context.provider_session_ids.is_empty();
    let exact = manifest.provider_source_key == context.provider_source_key;
    if manifest.effective_logging != ConversationLoggingSetting::Enabled
        || !same_sessions
        || !(exact || corroborated_legacy)
    {
        return Ok(None);
    }
    let binding = hash_json(&(
        &manifest.agent_id,
        &manifest.conversation_id,
        &manifest.provider,
        &manifest.provider_source_key,
        &manifest.provider_session_ids,
        &manifest.created_at,
    ))?;
    Ok(Some(Owned {
        manifest,
        stamp,
        directory: directory.to_owned(),
        binding,
        records: entry.records,
    }))
}

fn read_bounded(file: &mut File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(FRAME_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > FRAME_LIMIT {
        return Err(invalid("chat archive read exceeds record limit"));
    }
    Ok(bytes)
}

fn empty_page(agent_id: &str, progress: &str) -> AgentChatPage {
    AgentChatPage {
        session_id: agent_id.into(),
        conversation_id: None,
        generation: None,
        source_epoch: None,
        recent_admission: None,
        archive_read_ready: false,
        revision: format!("{TOKEN}:{progress}"),
        presentation: None,
        events: Vec::new(),
        next_before: None,
        unchanged: false,
        reset: false,
        older_page: false,
        progress: progress.into(),
        aliases: Vec::new(),
        removed_ids: Vec::new(),
        detail: None,
        revalidated_ids: None,
        bytes_read: 0,
        records_decoded: 0,
    }
}

fn token(owned: &Owned, positions: &ConversationPositions, row: &RowPosition) -> String {
    format!(
        "{TOKEN}:{}:{}:{}",
        owned.binding,
        row.seq,
        positions.row_proof(row)
    )
}

fn locate<'a>(
    value: &str,
    cohort: &[Owned],
    indexes: &'a HashMap<String, ConversationPositions>,
) -> Option<(usize, usize, &'a RowPosition)> {
    let mut parts = value.split(':');
    if parts.next()? != TOKEN {
        return None;
    }
    let binding = parts.next()?;
    let seq = parts.next()?.parse::<u64>().ok()?;
    let proof = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let owner = cohort.iter().position(|item| item.binding == binding)?;
    let index = indexes.get(&cohort[owner].manifest.conversation_id)?;
    let start = index.rows.partition_point(|row| row.seq < seq);
    index
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take_while(|(_, row)| row.seq == seq)
        .find(|(_, row)| index.row_proof(row) == proof)
        .map(|(position, row)| (owner, position, row))
}

impl Positions {
    /// Read exact saved observations for the operational completion consumer.
    /// A missing or unfinished member must never turn an earlier reply into
    /// the apparent final answer. Limit this read to the active turn window.
    pub(super) fn completion_events(
        &mut self,
        context: &ConversationArchiveContext,
        current: &[AgentChatEvent],
    ) -> io::Result<Vec<AgentChatEvent>> {
        let listing_path = index_path(&context.agent_id)?;
        let mut reads = IndexReads::default();
        self.index.refresh(&listing_path, &mut reads, listing)?;
        if self.index.pending()
            || (self.index.frames.is_empty() && has_saved_manifest(&listing_path)?)
        {
            return Err(invalid("chat completion archive catalog is pending"));
        }
        let mut seen = HashSet::new();
        let mut active = None;
        for (_, entry) in self.index.frames.iter().rev() {
            if !seen.insert(&entry.id) || entry.provider != context.provider {
                continue;
            }
            if entry.agent != context.agent_id {
                return Err(invalid("foreign chat completion archive catalog entry"));
            }
            let directory = conversation_dir(&context.agent_id, &entry.id)?;
            let Some(owned) = select_manifest(&directory, entry, context)? else {
                continue;
            };
            if owned.manifest.status == wardian_core::conversations::ConversationStatus::Open
                && active.replace(owned).is_some()
            {
                return Err(invalid("ambiguous open chat completion archive"));
            }
        }
        let Some(owned) = active else {
            return Ok(Vec::new());
        };
        let capture_path = capture_state_path(&context.agent_id)?;
        let (source, capture_stamp) = source_admission(context, &capture_path)?;
        if source
            .as_ref()
            .is_some_and(|source| matches!(source.status.as_str(), "pending" | "incomplete"))
        {
            return Err(invalid("chat completion native capture is pending"));
        }
        let epoch = source
            .as_ref()
            .map(|item| hash_json(&item.native_identity))
            .transpose()?;
        let id = &owned.manifest.conversation_id;
        let index = self.conversations.entry(id.clone()).or_default();
        index.refresh(&owned.directory, id, &mut reads)?;
        if index.incomplete
            || index.events.pending()
            || index.narrative.pending()
            || index.turns.pending()
            || owned.records.max(owned.manifest.record_count) > index.narrative.frames.len() as u64
        {
            return Err(invalid("chat completion archive payload is pending"));
        }
        let mut reader = RowReader::open(&owned, index)?;
        let mut metrics = empty_page(&context.agent_id, "ready");
        let mut events = Vec::new();
        let mut payload_bytes = 0;
        for row in index.rows.iter().rev().take(80) {
            let (mut event, record, _) = reader.read(
                row,
                context,
                source.as_ref(),
                epoch.as_deref(),
                &mut metrics,
            )?;
            if event.kind == wardian_core::models::chat::AgentChatEventKind::Message {
                if let Some(reference) = body_reference(&event)? {
                    let Some(Body::Artifact(file, path, stamp, _)) =
                        open_body(&owned, &event, &record)?
                    else {
                        return Err(invalid("chat completion row has no exact artifact binding"));
                    };
                    let body = Body::Artifact(file, path, stamp, String::new())
                        .full(PAGE_READ_LIMIT - payload_bytes)?;
                    verify_completion_payload(&event, &record, reference, &body, current)?;
                    payload_bytes += body.len() as u64;
                    event.text = Some(body);
                    if let Some(metadata) = event.metadata.as_object_mut() {
                        metadata.remove("text_artifact_refs");
                        metadata.remove("text_excerpt");
                    }
                } else if event.text.is_none()
                    && (record.text.is_some()
                        || record.excerpt.is_some()
                        || !record.artifact_refs.is_empty())
                {
                    return Err(invalid("chat completion row has no exact payload binding"));
                }
            }
            let request = event.kind == wardian_core::models::chat::AgentChatEventKind::Message
                && event.role == Some(wardian_core::models::chat::AgentChatRole::User)
                && event
                    .text
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty());
            event.metadata["conversation_archive_id"] = json!(id);
            events.push(event);
            if metrics.bytes_read as u64 > PAGE_READ_LIMIT {
                return Err(invalid("chat completion archive evidence exceeds budget"));
            }
            if request {
                break;
            }
        }
        if !index.current(&owned.directory)?
            || !self.index.current(&listing_path)?
            || path_stamp(&owned.directory.join("manifest.json"))?.as_ref() != Some(&owned.stamp)
            || (context.provider_source_key.is_some()
                && path_stamp(&capture_path)? != capture_stamp)
        {
            return Err(invalid("chat completion archive changed during read"));
        }
        events.reverse();
        Ok(events)
    }

    pub(super) fn read(
        &mut self,
        context: &ConversationArchiveContext,
        request: &Request,
    ) -> io::Result<AgentChatPage> {
        let result = self.read_indexed(context, request);
        if result.is_err() {
            *self = Self::default();
        }
        result
    }

    fn read_indexed(
        &mut self,
        context: &ConversationArchiveContext,
        request: &Request,
    ) -> io::Result<AgentChatPage> {
        let listing_path = index_path(&context.agent_id)?;
        let mut index_reads = IndexReads::default();
        self.index
            .refresh(&listing_path, &mut index_reads, listing)?;
        let mut entries = BTreeMap::new();
        for (_, entry) in &self.index.frames {
            entries.insert(entry.id.as_str(), entry);
        }
        let mut entries: Vec<_> = entries.into_values().collect();
        entries.sort_by(|a, b| (&a.started, &a.id).cmp(&(&b.started, &b.id)));
        let mut cohort = Vec::new();
        for entry in entries {
            if entry.agent != context.agent_id {
                return Err(invalid("chat archive index contains a foreign agent"));
            }
            if entry.provider != context.provider {
                continue;
            }
            let directory = conversation_dir(&context.agent_id, &entry.id)?;
            if let Some(owned) = select_manifest(&directory, entry, context)? {
                self.conversations
                    .entry(entry.id.clone())
                    .or_default()
                    .refresh(&directory, &entry.id, &mut index_reads)?;
                cohort.push(owned);
            }
        }
        let admitted: HashSet<_> = cohort
            .iter()
            .map(|owned| &owned.manifest.conversation_id)
            .collect();
        self.conversations.retain(|id, _| admitted.contains(id));
        let reset_requested =
            request.before.is_some() || request.detail.is_some() || request.revision.is_some();
        if cohort.is_empty() {
            let mut progress = "archive_scope_unavailable";
            if self.index.frames.is_empty() {
                progress = if self.index.pending() {
                    "archive_tail_pending"
                } else {
                    "archive_empty"
                };
                if has_saved_manifest(&listing_path)? {
                    progress = "archive_catalog_unavailable";
                }
            }
            let mut page = empty_page(&context.agent_id, progress);
            page.reset = reset_requested;
            page.bytes_read = index_reads.bytes;
            page.records_decoded = index_reads.records;
            return Ok(page);
        }
        let mut page = empty_page(&context.agent_id, "ready");
        let capture_path = capture_state_path(&context.agent_id)?;
        let (source, capture_stamp) = source_admission(context, &capture_path)?;
        let relevant_source = source.as_ref();
        let epoch = relevant_source
            .map(|source| hash_json(&source.native_identity))
            .transpose()?;
        for owned in &mut cohort {
            owned.binding = hash_json(&(
                &owned.binding,
                &epoch,
                relevant_source.map(|source| source.policy_generation),
            ))?;
        }
        let newest = cohort.last().expect("nonempty archive cohort");
        let stream_identities = cohort
            .iter()
            .map(|item| {
                let index = &self.conversations[&item.manifest.conversation_id];
                (
                    &item.binding,
                    index.events.stamp.as_ref().map(|stamp| &stamp.identity),
                    index.narrative.stamp.as_ref().map(|stamp| &stamp.identity),
                )
            })
            .collect::<Vec<_>>();
        let generation = hash_json(&(
            &newest.binding,
            &epoch,
            request.logging_enabled,
            relevant_source.map(|source| source.policy_generation),
            stream_identities,
        ))?;
        let revision_inputs = cohort
            .iter()
            .map(|item| {
                let index = &self.conversations[&item.manifest.conversation_id];
                (
                    &item.binding,
                    &item.stamp,
                    item.records,
                    &index.events.stamp,
                    &index.narrative.stamp,
                    &index.turns.stamp,
                )
            })
            .collect::<Vec<_>>();
        let mut artifacts = Vec::new();
        let mut artifact_paths = Vec::new();
        let mut artifact_missing = false;
        let mut artifact_unverified = false;
        'artifacts: for owned in &cohort {
            let index = &self.conversations[&owned.manifest.conversation_id];
            for reference in &index.artifacts {
                if artifacts.len() == ARTIFACT_PROBE_LIMIT {
                    artifact_unverified = true;
                    break 'artifacts;
                }
                let path = artifact_path(&owned.directory, reference).ok();
                let proof = path
                    .as_deref()
                    .and_then(|path| path_stamp(path).ok().flatten())
                    .filter(|stamp| stamp.len != 0);
                artifact_missing |= proof.is_none();
                artifact_paths.push(path);
                artifacts.push((&owned.binding, reference, proof));
            }
        }
        page.revision = format!(
            "{TOKEN}:{generation}:{}",
            hash_json(&(&generation, revision_inputs, &artifacts))?
        );
        page.reset = request
            .revision
            .as_ref()
            .is_some_and(|r| !r.starts_with(&format!("{TOKEN}:{generation}:")));
        page.conversation_id = Some(newest.manifest.conversation_id.clone());
        page.generation = Some(generation);
        page.source_epoch = epoch.clone();
        let incomplete = cohort.iter().any(|item| {
            let index = &self.conversations[&item.manifest.conversation_id];
            index.incomplete
                || item.records.max(item.manifest.record_count)
                    > index.narrative.frames.len() as u64
        });
        let pending = self.index.pending()
            || cohort.iter().any(|item| {
                let index = &self.conversations[&item.manifest.conversation_id];
                index.events.pending() || index.narrative.pending() || index.turns.pending()
            });
        let unverified_history =
            relevant_source.is_some_and(|source| source.unknown_before_offset.unwrap_or(0) > 0);
        page.archive_read_ready = !incomplete
            && !pending
            && !unverified_history
            && !artifact_missing
            && !artifact_unverified;
        page.progress = if incomplete || artifact_missing {
            "archive_payload_missing"
        } else if pending {
            "archive_tail_pending"
        } else if unverified_history || artifact_unverified {
            "archive_history_unverified"
        } else if request.logging_enabled {
            "ready"
        } else {
            "logging_disabled"
        }
        .into();

        if let Some(detail) = request.detail.as_deref() {
            self.read_detail(
                detail,
                &cohort,
                context,
                relevant_source,
                epoch.as_deref(),
                &mut page,
            )?;
        } else {
            self.read_window(
                &cohort,
                context,
                relevant_source,
                epoch.as_deref(),
                request,
                &mut page,
            )?;
        }
        if !self.index.current(&listing_path)?
            || (context.provider_source_key.is_some()
                && path_stamp(&capture_path)? != capture_stamp)
            || (page.archive_read_ready
                && artifact_paths
                    .iter()
                    .zip(&artifacts)
                    .any(|(path, (_, _, stamp))| {
                        path.as_deref()
                            .and_then(|path| path_stamp(path).ok().flatten())
                            .as_ref()
                            != stamp.as_ref()
                    }))
            || cohort
                .iter()
                .try_fold(false, |changed, owned| -> io::Result<bool> {
                    let manifest = optional_file(&owned.directory.join("manifest.json"))?;
                    Ok(changed
                        || manifest.as_ref().map(Stamp::of).transpose()?.as_ref()
                            != Some(&owned.stamp)
                        || !self.conversations[&owned.manifest.conversation_id]
                            .current(&owned.directory)?)
                })?
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "chat archive changed during read",
            ));
        }
        page.bytes_read += index_reads.bytes;
        page.records_decoded += index_reads.records;
        Ok(page)
    }

    fn read_window(
        &self,
        cohort: &[Owned],
        context: &ConversationArchiveContext,
        source: Option<&SourceAdmission>,
        epoch: Option<&str>,
        request: &Request,
        page: &mut AgentChatPage,
    ) -> io::Result<()> {
        let cursor = request
            .before
            .as_deref()
            .and_then(|value| locate(value, cohort, &self.conversations));
        if let Some((owner, _, row)) = cursor {
            let owned = &cohort[owner];
            let index = &self.conversations[&owned.manifest.conversation_id];
            // A cached offset never grants Older authority on its own.
            RowReader::open(owned, index)?.read(row, context, source, epoch, page)?;
        }
        if request.before.is_some() {
            page.reset = cursor.is_none();
            page.older_page = cursor.is_some();
        }
        let last = cursor.map_or(cohort.len() - 1, |(owner, _, _)| owner);
        let mut encoded = 2048usize;
        let mut frontier = None;
        let mut more = false;
        let mut displayed = HashSet::new();
        let mut body_evidence = Vec::new();
        let mut turn_rows = Vec::new();
        'cohort: for owner in (0..=last).rev() {
            let owned = &cohort[owner];
            let index = &self.conversations[&owned.manifest.conversation_id];
            let end = cursor
                .filter(|(cursor_owner, _, _)| *cursor_owner == owner)
                .map_or(index.rows.len(), |(_, offset, _)| offset);
            if end == 0 {
                continue;
            }
            let mut reader = RowReader::open(owned, index)?;
            for row in index.rows[..end].iter().rev() {
                if page.events.len() == request.limit
                    || (page.bytes_read as u64 >= PAGE_READ_LIMIT && !page.events.is_empty())
                {
                    more = true;
                    break 'cohort;
                }
                let (event, record, turn) = reader.read(row, context, source, epoch, page)?;
                if !displayed.insert(event.id.clone()) {
                    return Err(invalid("ambiguous event identity across chat archives"));
                }
                let mut visible = header(&event);
                for key in [
                    "chat_source_ref",
                    "chat_source_epoch",
                    "chat_source_start",
                    "chat_source_end",
                    "chat_source_admission",
                    "chat_provisional",
                    "chat_display_physical_id",
                    "chat_display_member_ids",
                ] {
                    if let Some(metadata) = visible.metadata.as_object_mut() {
                        metadata.remove(key);
                    }
                }
                visible.metadata["chat_archive_positioned"] = json!(true);
                if visible.text.is_none() {
                    visible.text = event.metadata["text_excerpt"]
                        .as_str()
                        .or_else(|| {
                            (record.event_refs.len() == 1)
                                .then_some(record.excerpt.as_deref())
                                .flatten()
                        })
                        .map(|text| clip(text, PREVIEW_BYTES));
                }
                let key = token(owned, index, row);
                match open_body(owned, &event, &record) {
                    Ok(Some(body)) => {
                        let binding = body.binding()?;
                        visible.metadata["chat_body_unavailable"] = json!(false);
                        visible.metadata["chat_body_binding"] = json!(binding);
                        visible.metadata["chat_detail_ref"] = json!(format!("{key}:{binding}:0"));
                    }
                    Ok(None) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                        ) =>
                    {
                        visible.metadata["chat_body_unavailable"] = json!(true);
                        page.progress = "archive_payload_missing".into();
                        page.archive_read_ready = false;
                    }
                    Err(error) => return Err(error),
                }
                let size = serde_json::to_vec(&visible)
                    .map_err(io::Error::other)?
                    .len()
                    + 1;
                if encoded + size > IPC_BYTES {
                    if page.events.is_empty() {
                        return Err(invalid("chat archive event exceeds response budget"));
                    }
                    more = true;
                    break 'cohort;
                }
                encoded += size;
                body_evidence.push((
                    key.clone(),
                    visible.metadata["chat_body_binding"]
                        .as_str()
                        .map(str::to_owned),
                    visible.metadata["chat_body_unavailable"] == true,
                ));
                if let Some(turn) = turn {
                    turn_rows.push(presentation::Occurrence {
                        owner: owned.binding.clone(),
                        key: key.clone(),
                        sequence: row.seq,
                        event,
                        turn,
                        body: visible.metadata["chat_body_binding"]
                            .as_str()
                            .map(str::to_owned),
                        body_unavailable: visible.metadata["chat_body_unavailable"] == true,
                    });
                }
                frontier = Some(key);
                page.events.push(visible);
            }
        }
        page.events.reverse();
        page.next_before = frontier.filter(|_| more);
        let generation = page.generation.as_deref().unwrap_or_default();
        let base_revision = page.revision.clone();
        // A fully checked archive revision spans Recent and Older. An
        // incomplete cohort binds the displayed evidence it actually proved.
        if !page.archive_read_ready {
            page.revision = format!(
                "{TOKEN}:{generation}:{}",
                hash_json(&(&page.revision, body_evidence, &page.progress))?
            );
        }
        turn_rows.reverse();
        let mut positioned_headers = page.events.clone();
        let extra = presentation::window(
            &turn_rows,
            &base_revision,
            &page.revision,
            &mut positioned_headers,
        )?;
        if let Some(extra) = extra {
            if serde_json::to_vec(&(&positioned_headers, &extra))
                .map_err(io::Error::other)?
                .len()
                <= IPC_BYTES - 2048
            {
                page.events = positioned_headers;
                page.presentation = Some(extra);
            }
        }
        if request.before.is_none()
            && !page.reset
            && page.archive_read_ready
            && request.revision.as_deref() == Some(&page.revision)
        {
            page.unchanged = true;
            page.events.clear();
            page.presentation = None;
        }
        Ok(())
    }

    fn read_detail(
        &self,
        detail: &str,
        cohort: &[Owned],
        context: &ConversationArchiveContext,
        source: Option<&SourceAdmission>,
        epoch: Option<&str>,
        page: &mut AgentChatPage,
    ) -> io::Result<()> {
        if let Some(result) = self.read_step(detail, cohort, context, source, epoch, page)? {
            page.presentation = Some(result);
            return Ok(());
        }
        let parsed = detail.rsplit_once(':').and_then(|(rest, offset)| {
            let (key, binding) = rest.rsplit_once(':')?;
            Some((key, binding, offset.parse::<u64>().ok()?))
        });
        if let Some((key, binding, offset)) = parsed {
            if let Some((owner, _, row)) = locate(key, cohort, &self.conversations) {
                let owned = &cohort[owner];
                let index = &self.conversations[&owned.manifest.conversation_id];
                let (event, record, _) =
                    RowReader::open(owned, index)?.read(row, context, source, epoch, page)?;
                let body = match open_body(owned, &event, &record) {
                    Ok(body) => body,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                        ) =>
                    {
                        None
                    }
                    Err(error) => return Err(error),
                };
                if let Some(body) = body {
                    if body.binding()? == binding {
                        let (text, complete) = body.chunk(offset)?;
                        let next = (!complete)
                            .then(|| format!("{key}:{binding}:{}", offset + text.len() as u64));
                        page.detail = Some(AgentChatDetail {
                            event_id: event.id,
                            text,
                            next,
                            complete,
                        });
                        return Ok(());
                    }
                }
            }
        }
        page.reset = true;
        page.unchanged = false;
        page.progress = "archive_detail_unavailable".into();
        Ok(())
    }

    fn read_step(
        &self,
        reference: &str,
        cohort: &[Owned],
        context: &ConversationArchiveContext,
        source: Option<&SourceAdmission>,
        epoch: Option<&str>,
        page: &mut AgentChatPage,
    ) -> io::Result<Option<wardian_core::models::chat_presentation::ChatPresentation>> {
        use wardian_core::models::chat::AgentChatEventKind as Kind;
        use wardian_core::models::chat_presentation::*;
        let parts = reference.split(':').collect::<Vec<_>>();
        let ["step", TOKEN, owner, call_seq, call_proof, result_seq, result_proof, body_proof, base, displayed] =
            parts.as_slice()
        else {
            return Ok(None);
        };
        if page.revision.rsplit(':').next() != Some(*base)
            || displayed.len() != 64
            || !displayed.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Ok(None);
        }
        let Some((call_owner, _, call_row)) = locate(
            &format!("{TOKEN}:{owner}:{call_seq}:{call_proof}"),
            cohort,
            &self.conversations,
        ) else {
            return Ok(None);
        };
        let Some((result_owner, _, result_row)) = locate(
            &format!("{TOKEN}:{owner}:{result_seq}:{result_proof}"),
            cohort,
            &self.conversations,
        ) else {
            return Ok(None);
        };
        if call_owner != result_owner || call_row.seq >= result_row.seq {
            return Ok(None);
        }
        let owned = &cohort[call_owner];
        let index = &self.conversations[&owned.manifest.conversation_id];
        let mut reader = RowReader::open(owned, index)?;
        let (call, _, call_turn) = reader.read(call_row, context, source, epoch, page)?;
        let (result, record, result_turn) =
            reader.read(result_row, context, source, epoch, page)?;
        let (Some(turn), Some(id)) = (call_turn, presentation::call_id(&call)) else {
            return Ok(None);
        };
        if Some(&turn) != result_turn.as_ref()
            || call.kind != Kind::ToolCall
            || result.kind != Kind::ToolResult
            || presentation::call_id(&result) != Some(id)
            || matches!(
                presentation::step_status(&result),
                ChatStepStatus::Running | ChatStepStatus::ActionRequired
            )
        {
            return Ok(None);
        }
        let start = index.rows.partition_point(|row| row.seq < turn.seq_start);
        let end = index.rows.partition_point(|row| row.seq <= turn.seq_end);
        if end - start > 80 {
            return Ok(None);
        }
        let (mut calls, mut results) = (0, 0);
        for row in &index.rows[start..end] {
            let (member, _, _) = reader.read(row, context, source, epoch, page)?;
            if presentation::call_id(&member) == Some(id) {
                calls += usize::from(member.kind == Kind::ToolCall);
                results += usize::from(member.kind == Kind::ToolResult);
            }
        }
        if calls != 1 || results != 1 {
            return Ok(None);
        }
        let body = match open_body(owned, &result, &record) {
            Ok(body) => body,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        let proof = body
            .as_ref()
            .map(Body::binding)
            .transpose()?
            .unwrap_or(hash_json(&result.text)?);
        if proof != *body_proof {
            return Ok(None);
        }
        let (text, total_bytes) = match body {
            Some(Body::Artifact(file, path, stamp, _)) => artifact_tail(file, &path, stamp)?,
            _ => {
                let output = result.text.as_deref().unwrap_or_default();
                let mut start = output.len().saturating_sub(CHAT_STEP_OUTPUT_BYTES);
                while !output.is_char_boundary(start) {
                    start += 1;
                }
                (output[start..].to_owned(), output.len() as u64)
            }
        };
        let input = super::chat_read::tool_input_body(&call)
            .or_else(|| call.command.clone())
            .or_else(|| super::chat_read::tool_input_body(&result));
        let input_truncated = input
            .as_ref()
            .is_some_and(|text| text.len() > CHAT_STEP_INPUT_BYTES);
        let input_text = input.map(|text| clip(&text, CHAT_STEP_INPUT_BYTES));
        let complete = total_bytes == text.len() as u64;
        let generation = page.generation.as_deref().unwrap_or_default();
        let revision = format!("{TOKEN}:{generation}:{displayed}");
        let mut value = presentation::envelope();
        value.detail = Some(ChatStepDetail {
            step_key: presentation::item_key(&owned.binding, &call),
            revision: revision.clone(),
            format: if complete && serde_json::from_str::<serde_json::Value>(&text).is_ok() {
                ChatOutputFormat::Json
            } else {
                ChatOutputFormat::Text
            },
            text,
            input_text,
            input_truncated,
            result_summary: None,
            pending: false,
            unavailable: false,
            complete,
            truncated_before: !complete,
            total_bytes: Some(total_bytes),
        });
        page.revision = revision;
        page.reset = false;
        presentation::encode(value).map(Some)
    }
}

fn has_saved_manifest(listing: &Path) -> io::Result<bool> {
    let Some(root) = listing.parent() else {
        return Ok(false);
    };
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("manifest.json").is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn artifact_tail(mut file: File, path: &Path, stamp: Stamp) -> io::Result<(String, u64)> {
    use wardian_core::models::chat_presentation::CHAT_STEP_OUTPUT_BYTES;
    let start = stamp.len.saturating_sub(CHAT_STEP_OUTPUT_BYTES as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(stamp.len - start + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != stamp.len - start
        || Stamp::of(&file)? != stamp
        || path_stamp(path)?.as_ref() != Some(&stamp)
    {
        return Err(invalid("chat step artifact changed during read"));
    }
    let first = if start == 0 {
        0
    } else {
        bytes
            .iter()
            .take(3)
            .take_while(|byte| **byte & 0xc0 == 0x80)
            .count()
    };
    let text = std::str::from_utf8(&bytes[first..])
        .map_err(io::Error::other)?
        .to_owned();
    Ok((text, stamp.len))
}

struct RowReader<'a> {
    owned: &'a Owned,
    index: &'a ConversationPositions,
    events: File,
    narrative: File,
    turns: Option<File>,
}

impl<'a> RowReader<'a> {
    fn open(owned: &'a Owned, index: &'a ConversationPositions) -> io::Result<Self> {
        Ok(Self {
            owned,
            index,
            events: plain_file(&owned.directory.join("events.jsonl"))?,
            narrative: plain_file(&owned.directory.join("conversation.jsonl"))?,
            turns: optional_file(&owned.directory.join("turns.jsonl"))?,
        })
    }

    fn read(
        &mut self,
        row: &RowPosition,
        context: &ConversationArchiveContext,
        source: Option<&SourceAdmission>,
        epoch: Option<&str>,
        page: &mut AgentChatPage,
    ) -> io::Result<(
        AgentChatEvent,
        ConversationNarrativeRecord,
        Option<ConversationTurnRecord>,
    )> {
        let event = &self.index.events.frames[row.event].0;
        let narrative = &self.index.narrative.frames[row.narrative].0;
        let mut payload: AgentChatEvent =
            serde_json::from_slice(&event.load(&mut self.events)?).map_err(io::Error::other)?;
        let record: ConversationNarrativeRecord =
            serde_json::from_slice(&narrative.load(&mut self.narrative)?)
                .map_err(io::Error::other)?;
        if payload.session_id != context.agent_id
            || (payload.provider != context.provider
                && !(payload.provider == "unknown"
                    && payload.metadata["generated"] == true
                    && payload.id
                        == format!(
                            "generated:{}:{}",
                            self.owned.manifest.conversation_id, record.seq
                        )))
            || !record.event_refs.contains(&payload.id)
            || record.seq != row.seq
        {
            return Err(invalid(
                "chat archive event is outside its narrative ownership",
            ));
        }
        if payload.metadata["provider_session_id"]
            .as_str()
            .is_some_and(|id| {
                !self
                    .owned
                    .manifest
                    .provider_session_ids
                    .iter()
                    .any(|owned| owned == id)
            })
        {
            return Err(invalid(
                "chat archive event has a conflicting provider session",
            ));
        }
        if payload.metadata["provider_log"] == true {
            if let Some(event_epoch) = payload.metadata["chat_source_epoch"].as_str() {
                let source =
                    source.ok_or_else(|| invalid("chat archive source proof is missing"))?;
                let coordinate = payload.metadata["chat_source_start"]
                    .as_u64()
                    .zip(payload.metadata["chat_source_end"].as_u64());
                if Some(event_epoch) != epoch
                    || !coordinate.is_some_and(|(start, end)| {
                        start >= source.unknown_before_offset.unwrap_or(0)
                            && start < end
                            && end <= source.committed_offset
                            && source.open_disabled_from.is_none_or(|cutoff| end <= cutoff)
                            && !source
                                .disabled_spans
                                .iter()
                                .any(|span| span.start < end && start < span.end)
                    })
                {
                    return Err(invalid(
                        "chat archive source coordinates are outside the admitted interval",
                    ));
                }
            }
        }
        page.bytes_read += (event.len + narrative.len) as usize;
        page.records_decoded += 2;
        let turn =
            if let (Some(file), Some(frame)) = (&mut self.turns, self.index.turn_frame(row.seq)) {
                let turn: ConversationTurnRecord =
                    serde_json::from_slice(&frame.load(file)?).map_err(io::Error::other)?;
                page.bytes_read += frame.len as usize;
                page.records_decoded += 1;
                if payload.turn_id.is_none() {
                    payload.turn_id = Some(turn.turn_key.clone());
                }
                Some(turn)
            } else {
                None
            };
        Ok((payload, record, turn))
    }
}

enum Body {
    Inline(String),
    Artifact(File, PathBuf, Stamp, String),
}

impl Body {
    fn full(self, remaining: u64) -> io::Result<String> {
        match self {
            Self::Inline(text) if text.len() as u64 <= remaining => Ok(text),
            Self::Artifact(mut file, path, stamp, prefix)
                if stamp.len.saturating_add(prefix.len() as u64) <= remaining =>
            {
                let mut text = prefix;
                let start = text.len();
                (&mut file).take(stamp.len + 1).read_to_string(&mut text)?;
                if (text.len() - start) as u64 != stamp.len
                    || Stamp::of(&file)? != stamp
                    || path_stamp(&path)?.as_ref() != Some(&stamp)
                {
                    return Err(invalid("chat completion artifact changed during read"));
                }
                Ok(text)
            }
            _ => Err(invalid("chat completion payload exceeds budget")),
        }
    }

    fn binding(&self) -> io::Result<String> {
        match self {
            Self::Inline(text) => Ok(digest(text.as_bytes())),
            Self::Artifact(_, _, stamp, prefix) => hash_json(&(stamp, digest(prefix.as_bytes()))),
        }
    }

    fn chunk(self, offset: u64) -> io::Result<(String, bool)> {
        match self {
            Self::Inline(text) => {
                let start = usize::try_from(offset).map_err(io::Error::other)?;
                let tail = text
                    .get(start..)
                    .ok_or_else(|| invalid("invalid chat detail byte offset"))?;
                let chunk = clip(tail, BODY_BYTES);
                let complete = chunk.len() == tail.len();
                Ok((chunk, complete))
            }
            Self::Artifact(mut file, path, stamp, prefix) => {
                let prefix_len = prefix.len() as u64;
                if offset < prefix_len {
                    let tail = prefix
                        .get(offset as usize..)
                        .ok_or_else(|| invalid("invalid chat input byte offset"))?;
                    return Ok((clip(tail, BODY_BYTES), false));
                }
                let offset = offset - prefix_len;
                if offset >= stamp.len {
                    return Err(invalid("invalid chat artifact byte offset"));
                }
                file.seek(SeekFrom::Start(offset))?;
                let mut bytes = Vec::new();
                (&mut file)
                    .take((BODY_BYTES + 3) as u64)
                    .read_to_end(&mut bytes)?;
                let expected = (stamp.len - offset).min((BODY_BYTES + 3) as u64) as usize;
                if bytes.len() != expected
                    || Stamp::of(&file)? != stamp
                    || Stamp::of(&plain_file(&path)?)? != stamp
                {
                    return Err(invalid("chat artifact changed during detail read"));
                }
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(error)
                        if error.error_len().is_none()
                            && offset + (bytes.len() as u64) < stamp.len =>
                    {
                        std::str::from_utf8(&bytes[..error.valid_up_to()])
                            .map_err(io::Error::other)?
                    }
                    Err(error) => return Err(io::Error::other(error)),
                };
                let chunk = clip(text, BODY_BYTES);
                let complete = offset + chunk.len() as u64 == stamp.len;
                Ok((chunk, complete))
            }
        }
    }
}

fn open_body(
    owned: &Owned,
    event: &AgentChatEvent,
    record: &ConversationNarrativeRecord,
) -> io::Result<Option<Body>> {
    if let Some(reference) = body_reference(event)? {
        if !record.artifact_refs.iter().any(|owned| owned == reference) {
            return Err(invalid(
                "chat detail artifact is not bound to its narrative",
            ));
        }
        let path = artifact_path(&owned.directory, reference)?;
        let file = plain_file(&path)?;
        let stamp = Stamp::of(&file)?;
        if stamp.len == 0 {
            return Err(invalid("chat detail artifact is empty"));
        }
        let prefix = super::chat_read::tool_input_body(event)
            .map(|input| format!("{input}\n\n"))
            .unwrap_or_default();
        return Ok(Some(Body::Artifact(file, path, stamp, prefix)));
    }
    Ok(inline_body_text(event)
        .filter(|text| {
            text.len() > PREVIEW_BYTES || super::chat_read::tool_input_body(event).is_some()
        })
        .map(Body::Inline))
}

fn verify_completion_payload(
    event: &AgentChatEvent,
    record: &ConversationNarrativeRecord,
    reference: &str,
    body: &str,
    current: &[AgentChatEvent],
) -> io::Result<()> {
    if let Some(proof) = &record.text_artifact_proof {
        if proof.reference == reference
            && proof.bytes == body.len() as u64
            && proof.sha256 == digest(body.as_bytes())
        {
            return Ok(());
        }
        return Err(invalid(
            "chat completion artifact differs from its writer proof",
        ));
    }
    // A pre-proof artifact may contribute only when its exact native frame
    // is present in the completed capture and agrees byte for byte.
    let source = [
        "chat_source_ref",
        "chat_source_epoch",
        "chat_source_start",
        "chat_source_end",
    ];
    let mut observations = current.iter().filter(|live| {
        live.id == event.id
            && live.role == event.role
            && event.metadata["chat_source_ref"].is_string()
            && event.metadata["chat_source_epoch"].is_string()
            && event.metadata["chat_source_start"].is_u64()
            && event.metadata["chat_source_end"].is_u64()
            && source
                .iter()
                .all(|key| live.metadata[*key] == event.metadata[*key])
            && super::provenance::same_observation(event, live)
    });
    if observations
        .next()
        .is_some_and(|live| live.text.as_deref() == Some(body))
        && observations.next().is_none()
    {
        return Ok(());
    }
    Err(invalid(
        "chat completion artifact lacks exact writer or live proof",
    ))
}

fn body_reference(event: &AgentChatEvent) -> io::Result<Option<&str>> {
    let refs = event
        .metadata
        .get("text_artifact_refs")
        .filter(|value| !value.is_null())
        .or_else(|| event.metadata["archive_record"].get("artifact_refs"));
    match refs {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(refs)) if refs.is_empty() => Ok(None),
        Some(serde_json::Value::Array(refs)) if refs.len() == 1 => refs[0]
            .as_str()
            .map(Some)
            .ok_or_else(|| invalid("invalid chat detail artifact reference")),
        _ => Err(invalid("ambiguous chat detail artifacts")),
    }
}
fn artifact_path(directory: &Path, reference: &str) -> io::Result<PathBuf> {
    let path = Path::new(reference);
    if reference.contains(':')
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || reference
            .split(['/', '\\'])
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(invalid("unsafe chat detail artifact path"));
    }
    if path.components().count() == 1 {
        Ok(directory.join("artifacts").join(path))
    } else if path.starts_with("artifacts") {
        Ok(directory.join(path))
    } else {
        Err(invalid(
            "chat detail artifact is outside the conversation artifacts",
        ))
    }
}
