//! Independently framed source observations, never a canonical archive seal.
//! The existing background capture owner publishes recent rows before walking
//! older intervals. A restart resumes the last immutable checkpoint.
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use wardian_core::conversations::write_json_atomic;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

use super::chat_read::{locations, PAGE_ROWS};
use super::chat_read_store::{valid_ref, Store};
use crate::commands::chat::{
    conversation_archive_context_from_snapshot, AgentArchiveCaptureSnapshot,
};
use crate::commands::chat_recent_seed::{source_scope, SeedCheckpoint};

const SOURCE_INDEX_VERSION: u8 = 5;
// The normalizer keeps its existing 1 MiB cap; this bounded envelope also
// stores the pinned acquisition identity, policy spans and cursor metadata.
const REPLAY_STATE_MAX_BYTES: usize = 1024 * 1024 + 64 * 1024;
const REPLAY_STATE_CHUNK_BYTES: usize = 7 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct SourceHead {
    agent_id: String,
    source_key: Option<String>,
    generation: String,
    epoch: String,
    admission: String,
    root: Option<String>,
    #[serde(default)]
    logical: super::chat_logical_index::Index,
    #[serde(default)]
    logical_version: u8,
    #[serde(default)]
    invalidations: u64,
    target_extent: u64,
    modified: String,
    floor: u64,
    #[serde(default)]
    admitted_through: Option<u64>,
    before: Option<u64>,
    progress: String,
    #[serde(default)]
    counts_gapped: bool,
    #[serde(default)]
    framed: bool,
    #[serde(default)]
    replay_state: Option<String>,
    #[serde(default)]
    replay_settled: bool,
    #[serde(default)]
    replay_bytes_read: u64,
    #[serde(default)]
    replay_records_decoded: u64,
    #[serde(default)]
    replay_rows_admitted: u64,
}

#[derive(Serialize, Deserialize)]
struct SourceRow {
    agent_id: String,
    admission: String,
    key: String,
    event: AgentChatEvent,
    #[serde(default)]
    relation: Option<super::chat_logical_index::Relation>,
    #[serde(default)]
    removed_display_ids: Vec<String>,
    #[serde(default)]
    body_required: bool,
}

#[derive(Clone)]
pub(super) struct SourcePage {
    pub(super) reference: String,
    pub(super) generation: String,
    pub(super) epoch: String,
    pub(super) admission: String,
    pub(super) target_extent: u64,
    pub(super) progress: String,
    pub(super) counts_complete: bool,
    pub(super) events: Vec<AgentChatEvent>,
    pub(super) next_before: Option<String>,
    pub(super) cursor_frontier: Option<(u64, u64)>,
    pub(super) range_covered: bool,
    pub(super) reset: bool,
}

pub(super) struct RevalidatedRows {
    pub(super) generation: String,
    pub(super) epoch: String,
    pub(super) admission: String,
    pub(super) progress: String,
    pub(super) complete: bool,
    pub(super) events: Vec<AgentChatEvent>,
    pub(super) body_required: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize)]
struct ReplayCheckpoint {
    acquisition: crate::commands::provider_log_acquisition::ProviderLogCaptureState,
    anchor_floor: u64,
    pending_root: Option<String>,
    pending_before: Option<String>,
    pending_count: usize,
}

#[derive(Serialize, Deserialize)]
struct ReplayStateChunk {
    next: Option<String>,
    hex: String,
}

#[derive(Serialize, Deserialize)]
struct ReplayPendingRow {
    key: String,
    event: AgentChatEvent,
    body_required: bool,
}

fn indexing_revalidation(head: &SourceHead) -> RevalidatedRows {
    RevalidatedRows {
        generation: head.generation.clone(),
        epoch: head.epoch.clone(),
        admission: head.admission.clone(),
        progress: "indexing".into(),
        complete: false,
        events: Vec::new(),
        body_required: Vec::new(),
    }
}

fn save_replay_checkpoint(store: &Store, checkpoint: &ReplayCheckpoint) -> io::Result<String> {
    let bytes = serde_json::to_vec(checkpoint).map_err(io::Error::other)?;
    if bytes.len() > REPLAY_STATE_MAX_BYTES {
        return Err(io::Error::other("source replay state budget exhausted"));
    }
    let mut next = None;
    for chunk in bytes.chunks(REPLAY_STATE_CHUNK_BYTES).rev() {
        let hex = chunk
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        next = Some(store.put(&ReplayStateChunk { next, hex })?);
    }
    next.ok_or_else(|| io::Error::other("empty source replay checkpoint"))
}

fn load_replay_checkpoint(store: &mut Store, reference: &str) -> io::Result<ReplayCheckpoint> {
    let mut next = Some(reference.to_owned());
    let mut seen = std::collections::HashSet::new();
    let mut bytes = Vec::new();
    while let Some(reference) = next {
        if !valid_ref(&reference) || !seen.insert(reference.clone()) || seen.len() > 160 {
            return Err(io::Error::other("invalid source replay checkpoint chain"));
        }
        let chunk: ReplayStateChunk = store.read(&reference)?;
        let (pairs, remainder) = chunk.hex.as_bytes().as_chunks::<2>();
        if !remainder.is_empty() || chunk.hex.len() > REPLAY_STATE_CHUNK_BYTES * 2 {
            return Err(io::Error::other("invalid source replay checkpoint chunk"));
        }
        let mut decoded = Vec::with_capacity(pairs.len());
        for pair in pairs {
            let value = std::str::from_utf8(pair).map_err(io::Error::other)?;
            decoded.push(u8::from_str_radix(value, 16).map_err(io::Error::other)?);
        }
        if bytes.len().saturating_add(decoded.len()) > REPLAY_STATE_MAX_BYTES {
            return Err(io::Error::other("source replay state budget exhausted"));
        }
        bytes.extend(decoded);
        next = chunk.next;
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn replay_pending_key(row_key: &str) -> io::Result<String> {
    let (start, ordinal) = row_key
        .split_once(':')
        .ok_or_else(|| io::Error::other("invalid source replay row key"))?;
    let start = start.parse::<u64>().map_err(io::Error::other)?;
    let ordinal = ordinal.parse::<u64>().map_err(io::Error::other)?;
    Ok(format!(
        "{:020}:{:020}",
        u64::MAX - start,
        u64::MAX - ordinal
    ))
}

fn source_id_parts(agent_id: &str, source_id: &str) -> Option<(String, String, u64)> {
    if source_id.len() > 512 {
        return None;
    }
    let suffix = source_id.strip_prefix(&format!("source:{agent_id}:"))?;
    let mut parts = suffix.split(':');
    let epoch = parts.next()?;
    let offset = parts.next()?;
    let raw_digest = parts.next()?;
    let ordinal = parts.next()?;
    if parts.next().is_some()
        || epoch.len() != 64
        || !epoch
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || raw_digest.len() != 64
        || !raw_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let offset_value = offset.parse::<u64>().ok()?;
    let ordinal_value = ordinal.parse::<u64>().ok()?;
    if offset_value.to_string() != offset || ordinal_value.to_string() != ordinal {
        return None;
    }
    Some((
        epoch.to_owned(),
        format!("{offset_value:020}:{ordinal_value:020}"),
        offset_value,
    ))
}

pub(super) fn valid_source_id(agent_id: &str, source_id: &str) -> bool {
    source_id_parts(agent_id, source_id).is_some()
}

/// Resolve only explicitly selected, backend-issued source observations. A
/// complete current index is required before any row can be acknowledged.
pub(super) fn revalidate_rows(
    snapshot: &AgentArchiveCaptureSnapshot,
    store: &mut Store,
    reference: &str,
    source_ids: &[String],
) -> io::Result<RevalidatedRows> {
    let context = conversation_archive_context_from_snapshot(snapshot);
    let (head, extent) = match admitted_head(snapshot, store, reference) {
        Ok(value) => value,
        Err(_) => {
            return Ok(RevalidatedRows {
                generation: String::new(),
                epoch: String::new(),
                admission: String::new(),
                progress: "source_changed".into(),
                complete: false,
                events: Vec::new(),
                body_required: Vec::new(),
            });
        }
    };
    let complete = coverage_complete(&head, extent);
    if !complete {
        return Ok(RevalidatedRows {
            generation: head.generation,
            epoch: head.epoch,
            admission: head.admission,
            progress: if matches!(
                head.progress.as_str(),
                "oversized_record" | "no_admitted_rows"
            ) {
                head.progress
            } else {
                "indexing".into()
            },
            complete: false,
            events: Vec::new(),
            body_required: Vec::new(),
        });
    }

    let policy = super::chat_read::source_policy(&context)?;
    let mut pending = std::collections::VecDeque::from(source_ids.to_vec());
    let mut rows = std::collections::BTreeMap::<String, SourceRow>::new();
    while let Some(source_id) = pending.pop_front() {
        if rows.contains_key(&source_id) {
            continue;
        }
        if rows.len() >= source_ids.len().saturating_mul(2) {
            return Err(io::Error::other("chat source revalidation unavailable"));
        }
        let Some((epoch, row_key, start)) = source_id_parts(&context.agent_id, &source_id) else {
            return Err(io::Error::other("chat source revalidation unavailable"));
        };
        if epoch != head.epoch {
            return Err(io::Error::other("chat source revalidation unavailable"));
        }
        let row_reference = match store.get(&head.root, &row_key) {
            Ok(Some(reference)) => reference,
            Ok(None) => return Err(io::Error::other("chat source revalidation unavailable")),
            Err(error) if error.to_string().contains("budget") => {
                return Ok(indexing_revalidation(&head));
            }
            Err(error) => return Err(error),
        };
        let row: SourceRow = match store.read(&row_reference) {
            Ok(row) => row,
            Err(error) if error.to_string().contains("budget") => {
                return Ok(indexing_revalidation(&head));
            }
            Err(error) => return Err(error),
        };
        let event_start = row.event.metadata["chat_source_start"].as_u64();
        let event_end = row.event.metadata["chat_source_end"].as_u64();
        if row.agent_id != context.agent_id
            || row.admission != head.admission
            || row.key != row_key
            || row.event.id.as_str() != source_id.as_str()
            || row.event.session_id != context.agent_id
            || row.event.metadata["chat_source_ref"].as_str() != Some(source_id.as_str())
            || row.event.metadata["chat_source_epoch"].as_str() != Some(head.epoch.as_str())
            || row.event.metadata["chat_source_admission"].as_str() != Some(head.admission.as_str())
            || event_start != Some(start)
            || event_end
                .is_none_or(|end| end <= start || end > extent || !policy.admits(start, end))
        {
            return Err(io::Error::other("chat source revalidation unavailable"));
        }
        if let Some(relation) = &row.relation {
            let unique_members = relation
                .members
                .iter()
                .collect::<std::collections::BTreeSet<_>>();
            if relation.members.len() != 2
                || unique_members.len() != 2
                || relation.id.is_empty()
                || relation.id.len() > 512
                || !relation.members.iter().any(|member| member == &source_id)
                || relation.members.iter().any(|member| {
                    source_id_parts(&context.agent_id, member)
                        .is_none_or(|(member_epoch, _, _)| member_epoch != head.epoch)
                })
            {
                return Err(io::Error::other("chat source revalidation unavailable"));
            }
            pending.extend(relation.members.iter().cloned());
        }
        rows.insert(source_id, row);
    }

    // A requested live row can have a completed physical partner that the
    // client has never seen. Resolve only that exact indexed member; never scan
    // neighboring rows or infer a match from visible text.
    for (source_id, row) in &rows {
        let Some(relation) = &row.relation else {
            continue;
        };
        for member in &relation.members {
            if rows
                .get(member)
                .is_none_or(|member_row| member_row.relation.as_ref() != Some(relation))
            {
                return Err(io::Error::other("chat source revalidation unavailable"));
            }
        }
        if !relation.members.iter().any(|member| member == source_id) {
            return Err(io::Error::other("chat source revalidation unavailable"));
        }
    }

    let mut events = Vec::with_capacity(rows.len());
    let mut body_required = Vec::new();
    for (source_id, mut row) in rows {
        if row.body_required {
            let source_ref = row.event.metadata["chat_source_ref"]
                .as_str()
                .ok_or_else(|| io::Error::other("source body binding absent"))?;
            body_required.push((source_id.clone(), source_ref.to_owned()));
        }
        row.event.metadata["chat_page_key"] = serde_json::json!(row.key);
        if let Some(relation) = row.relation {
            if !row.removed_display_ids.is_empty() {
                row.event.metadata["chat_display_removed_ids"] =
                    serde_json::json!(&row.removed_display_ids);
            }
            row.event.metadata["chat_display_physical_id"] = serde_json::json!(source_id);
            row.event.id = relation.id;
            row.event.metadata["chat_display_member_ids"] = serde_json::json!(relation.members);
        } else if let Some(metadata) = row.event.metadata.as_object_mut() {
            // Stored source rows also own their native detail binding. Clear
            // only projection fields that no longer have a current relation.
            metadata.remove("chat_display_physical_id");
            metadata.remove("chat_display_member_ids");
            metadata.remove("chat_display_removed_ids");
            if !row.removed_display_ids.is_empty() {
                metadata.insert(
                    "chat_display_removed_ids".into(),
                    serde_json::json!(&row.removed_display_ids),
                );
            }
        }
        events.push(row.event);
    }

    let current_pointer = pointer(&context.agent_id)?;
    let current_scope = admitted_head(snapshot, store, reference);
    let still_current = current_pointer.as_deref() == Some(reference)
        && current_scope.is_ok_and(|(current, current_extent)| {
            current.generation == head.generation
                && current.epoch == head.epoch
                && current.admission == head.admission
                && current_extent == extent
                && coverage_complete(&current, current_extent)
        });
    if !still_current {
        return Ok(RevalidatedRows {
            generation: head.generation,
            epoch: head.epoch,
            admission: head.admission,
            progress: "source_changed".into(),
            complete: false,
            events: Vec::new(),
            body_required: Vec::new(),
        });
    }
    events.sort_by_key(|event| event.sequence);
    Ok(RevalidatedRows {
        generation: head.generation,
        epoch: head.epoch,
        admission: head.admission,
        progress: head.progress,
        complete: true,
        events,
        body_required,
    })
}

/// A page certifies display relations only while its complete index head still
/// covers the current admitted source extent and remains the published head.
pub(super) fn current_complete(snapshot: &AgentArchiveCaptureSnapshot, page: &SourcePage) -> bool {
    if !page.counts_complete {
        return false;
    }
    let context = conversation_archive_context_from_snapshot(snapshot);
    if pointer(&context.agent_id).ok().flatten().as_deref() != Some(page.reference.as_str()) {
        return false;
    }
    source_scope(snapshot).is_ok_and(|(epoch, admission, extent, _)| {
        page.epoch == epoch && page.admission == admission && page.target_extent == extent
    })
}

pub(super) fn current_complete_scope(
    snapshot: &AgentArchiveCaptureSnapshot,
    reference: &str,
    generation: &str,
    epoch: &str,
) -> bool {
    let context = conversation_archive_context_from_snapshot(snapshot);
    if pointer(&context.agent_id).ok().flatten().as_deref() != Some(reference) {
        return false;
    }
    let Ok((_, directory)) = locations(&context.agent_id) else {
        return false;
    };
    let mut store = Store::new(&directory);
    admitted_head(snapshot, &mut store, reference).is_ok_and(|(head, extent)| {
        head.generation == generation && head.epoch == epoch && coverage_complete(&head, extent)
    })
}

pub(super) fn pointer(agent_id: &str) -> io::Result<Option<String>> {
    let path = locations(agent_id)?
        .0
        .with_file_name("chat-source-head.json");
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > 4096 {
        return Err(io::Error::other("oversized source head"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(4096).read_to_end(&mut bytes)?;
    let reference: Option<String> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if reference
        .as_deref()
        .is_some_and(|reference| !valid_ref(reference))
    {
        return Err(io::Error::other("invalid source head"));
    }
    Ok(reference)
}

fn key(event: &AgentChatEvent) -> io::Result<String> {
    let offset = event.metadata["chat_source_start"]
        .as_u64()
        .ok_or_else(|| io::Error::other("source row position absent"))?;
    let ordinal: u64 = event
        .id
        .rsplit(':')
        .next()
        .ok_or_else(|| io::Error::other("source row ordinal absent"))?
        .parse()
        .map_err(io::Error::other)?;
    Ok(format!("{offset:020}:{ordinal:020}"))
}

fn key_position(key: &str) -> Option<(u64, u64)> {
    let (offset, ordinal) = key.split_once(':')?;
    if offset.len() != 20 || ordinal.len() != 20 {
        return None;
    }
    Some((offset.parse().ok()?, ordinal.parse().ok()?))
}

/// One bounded forward checkpoint, called only by the coalesced capture owner
/// after privacy policy commit and before full canonical archive maintenance.
pub(crate) fn advance(snapshot: &AgentArchiveCaptureSnapshot) -> io::Result<bool> {
    let context = conversation_archive_context_from_snapshot(snapshot);
    let Ok((epoch, admission, extent, modified)) = source_scope(snapshot) else {
        return Ok(false);
    };
    let mut store = Store::bounded_writer(&locations(&context.agent_id)?.1);
    let previous =
        pointer(&context.agent_id)?.and_then(|reference| store.read::<SourceHead>(&reference).ok());
    let mut head = previous
        .filter(|head| {
            head.agent_id == context.agent_id
                && head.source_key == context.provider_source_key
                && head.epoch == epoch
                && head.admission == admission
                && extent >= head.target_extent
                && (extent > head.target_extent || head.modified == modified)
        })
        .unwrap_or_else(|| SourceHead {
            agent_id: context.agent_id.clone(),
            source_key: context.provider_source_key.clone(),
            generation: uuid::Uuid::new_v4().to_string(),
            epoch,
            admission,
            root: None,
            logical: Default::default(),
            logical_version: SOURCE_INDEX_VERSION,
            invalidations: 0,
            target_extent: extent,
            modified: modified.clone(),
            floor: 0,
            admitted_through: None,
            before: Some(extent),
            progress: "indexing".into(),
            counts_gapped: false,
            framed: false,
            replay_state: None,
            replay_settled: false,
            replay_bytes_read: 0,
            replay_records_decoded: 0,
            replay_rows_admitted: 0,
        });
    let policy = super::chat_read::source_policy(&context)?;
    if policy.admission_digest()? != head.admission {
        return Ok(false);
    }
    if head.logical_version < SOURCE_INDEX_VERSION {
        // Rebuild old reverse-window heads from the current admitted source
        // floor. The old root and relation proofs are never carried forward.
        head.root = None;
        head.logical = Default::default();
        head.logical_version = SOURCE_INDEX_VERSION;
        head.floor = policy.unknown_before;
        head.admitted_through = None;
        head.before = Some(extent);
        head.progress = "indexing".into();
        head.counts_gapped = false;
        head.framed = false;
        head.replay_state = None;
        head.replay_settled = false;
        head.replay_bytes_read = 0;
        head.replay_records_decoded = 0;
        head.replay_rows_admitted = 0;
        head.invalidations = head.invalidations.saturating_add(1);
    }
    head.logical.defer_narratives();

    let path = std::fs::canonicalize(
        snapshot
            .log_path
            .as_ref()
            .ok_or_else(|| io::Error::other("missing source path"))?,
    )?
    .to_string_lossy()
    .to_string();
    let mut checkpoint = match head.replay_state.as_deref() {
        Some(reference) => match load_replay_checkpoint(&mut store, reference) {
            Ok(checkpoint) => checkpoint,
            Err(error) if error.to_string().contains("budget") => return Ok(true),
            Err(_) => {
                head.root = None;
                head.logical = Default::default();
                head.replay_state = None;
                head.replay_settled = false;
                head.admitted_through = None;
                head.counts_gapped = false;
                head.framed = false;
                head.before = Some(extent);
                head.floor = policy.unknown_before;
                head.invalidations = head.invalidations.saturating_add(1);
                new_replay_checkpoint(snapshot, &context, &policy, extent, &path)?
            }
        },
        None => new_replay_checkpoint(snapshot, &context, &policy, extent, &path)?,
    };

    if coverage_complete(&head, extent)
        && checkpoint.pending_root.is_none()
        && checkpoint.acquisition.committed_offset >= replay_end(&policy, head.target_extent)
        && !checkpoint.acquisition.normalizer_has_pending_events()
        && checkpoint.acquisition.status != "incomplete"
    {
        return Ok(false);
    }

    if head.before.is_none() && head.logical.narrative_pending() {
        let updates = head.logical.activate_narratives(&mut store, 8)?;
        apply_updates(&mut store, &mut head, updates)?;
        head.progress = if head.logical.narrative_pending() {
            "indexing".into()
        } else if head.counts_gapped {
            "source_gap".into()
        } else {
            "provisional".into()
        };
        save_head_checkpoint(
            snapshot,
            &mut store,
            &mut head,
            &checkpoint,
            &policy,
            extent,
            &modified,
        )?;
        return Ok(head.logical.narrative_pending() || extent > head.target_extent);
    }

    let cursor_at_target = checkpoint.acquisition.committed_offset >= head.target_extent;
    let waiting_for_append = checkpoint.acquisition.reason.as_deref()
        == Some("provider_log_partial_record")
        || checkpoint.acquisition.normalizer_has_pending_events() && cursor_at_target;
    if extent > head.target_extent
        && checkpoint.pending_root.is_none()
        && (cursor_at_target || waiting_for_append)
    {
        // Keep each append as a fixed interval. An unfinished partial record
        // is revisited only after new bytes make its newline available.
        if cursor_at_target {
            // Keep the persistent history floor while advancing this replay interval.
            checkpoint.anchor_floor = checkpoint.acquisition.committed_offset;
        }
        head.target_extent = extent;
        head.modified = modified.clone();
        head.before = Some(extent);
        head.replay_settled = false;
        head.logical.defer_narratives();
        if checkpoint.acquisition.status == "pending"
            && checkpoint.acquisition.reason.as_deref() == Some("provider_log_partial_record")
        {
            checkpoint.acquisition.status = "pending".into();
            checkpoint.acquisition.reason = None;
        }
    }

    let mut boundary_bytes_read = 0;
    head.framed = replay_boundaries_framed(
        snapshot,
        &policy,
        &head.epoch,
        head.target_extent,
        &mut boundary_bytes_read,
    )?;
    head.replay_bytes_read = head.replay_bytes_read.saturating_add(boundary_bytes_read);
    if !head.framed {
        head.counts_gapped = true;
        head.progress = "source_framing_gap".into();
        save_head_checkpoint(
            snapshot,
            &mut store,
            &mut head,
            &checkpoint,
            &policy,
            extent,
            &modified,
        )?;
        return Ok(false);
    }
    if head.replay_rows_admitted == 0
        && head.replay_bytes_read == 0
        && head.replay_records_decoded == 0
    {
        head.floor = checkpoint.anchor_floor;
    }

    if head.counts_gapped
        && checkpoint.acquisition.status == "incomplete"
        && checkpoint.pending_root.is_none()
    {
        save_head_checkpoint(
            snapshot,
            &mut store,
            &mut head,
            &checkpoint,
            &policy,
            extent,
            &modified,
        )?;
        return Ok(false);
    }

    let pinned_replay_end = replay_end(&policy, head.target_extent);
    let waiting_on_partial_record =
        checkpoint.acquisition.reason.as_deref() == Some("provider_log_partial_record");
    let waiting_on_normalizer = checkpoint.pending_root.is_none()
        && checkpoint.acquisition.committed_offset >= pinned_replay_end
        && checkpoint.acquisition.normalizer_has_pending_events();
    if extent == head.target_extent
        && checkpoint.pending_root.is_none()
        && (waiting_on_partial_record || waiting_on_normalizer)
    {
        head.before = Some(head.target_extent);
        head.replay_settled = false;
        head.progress = if waiting_on_partial_record {
            "source_partial_record".into()
        } else {
            "source_waiting_for_append".into()
        };
        return Ok(false);
    }

    let mut made_progress = false;
    if checkpoint.pending_root.is_some() {
        let drained = drain_replay_rows(
            &mut store,
            &mut head,
            &mut checkpoint,
            &context,
            &policy,
            &path,
        )?;
        made_progress |= drained > 0;
    } else if checkpoint.acquisition.committed_offset < replay_end(&policy, head.target_extent) {
        let cursor = checkpoint.acquisition.committed_offset;
        if let Some(span) = policy
            .disabled
            .iter()
            .find(|span| span.start <= cursor && cursor < span.end)
        {
            if cursor != span.start || !policy.framed_starts.contains(&span.end) {
                head.counts_gapped = true;
                head.progress = "source_policy_boundary_unframed".into();
            } else if checkpoint.acquisition.normalizer_has_pending_events() {
                // A policy boundary cannot discard rows waiting for their
                // request root. Keep the interval uncertified instead.
                head.counts_gapped = true;
                head.progress = "source_pending_at_policy_boundary".into();
                save_head_checkpoint(
                    snapshot,
                    &mut store,
                    &mut head,
                    &checkpoint,
                    &policy,
                    extent,
                    &modified,
                )?;
                return Ok(false);
            } else {
                checkpoint.acquisition =
                    crate::commands::provider_log_acquisition::source_replay_state(
                        Path::new(&path),
                        &context.provider_source_key.clone().unwrap_or_default(),
                        &policy.identity,
                        span.end,
                    )?;
                checkpoint.anchor_floor = span.end;
                head.floor = span.end;
                made_progress = true;
            }
        } else {
            let segment_end = policy
                .disabled
                .iter()
                .filter(|span| span.start > cursor)
                .map(|span| span.start)
                .min()
                .unwrap_or_else(|| replay_end(&policy, head.target_extent))
                .min(replay_end(&policy, head.target_extent));
            if segment_end <= cursor {
                head.replay_settled = true;
                head.before = None;
            } else {
                let (batch, metrics) =
                    crate::commands::provider_log_acquisition::acquire_source_replay_batch(
                        crate::commands::provider_log_acquisition::ProviderLogSource {
                            session_id: &context.agent_id,
                            provider: &context.provider,
                            path: Path::new(&path),
                            provider_source_key: context
                                .provider_source_key
                                .as_deref()
                                .unwrap_or_default(),
                        },
                        crate::commands::provider_log_acquisition::SourceReplayRequest {
                            previous: checkpoint.acquisition.clone(),
                            expected_identity: policy.identity.clone(),
                            target_extent: segment_end,
                            anchor_floor: checkpoint.anchor_floor,
                        },
                    )?;
                checkpoint.acquisition = batch.next;
                head.replay_bytes_read = head
                    .replay_bytes_read
                    .saturating_add(metrics.native_bytes_read);
                head.replay_records_decoded = head
                    .replay_records_decoded
                    .saturating_add(metrics.records_decoded);
                made_progress |= checkpoint.acquisition.committed_offset > cursor;
                if checkpoint.acquisition.status == "incomplete" {
                    head.counts_gapped = true;
                    let reason = checkpoint
                        .acquisition
                        .reason
                        .as_deref()
                        .unwrap_or("source_decode_gap");
                    head.progress = if reason == "provider_log_record_exceeds_record_limit" {
                        "oversized_record".into()
                    } else {
                        reason.to_owned()
                    };
                } else {
                    let waiting_on_partial_record = checkpoint.acquisition.reason.as_deref()
                        == Some("provider_log_partial_record");
                    if batch.consumed_bytes > 0 || !waiting_on_partial_record {
                        queue_replay_events(
                            &mut store,
                            &mut head,
                            &mut checkpoint,
                            ReplayAdmissionContext {
                                snapshot,
                                context: &context,
                                policy: &policy,
                                path: &path,
                            },
                            batch.events,
                        )?;
                    }
                }
            }
        }
        if checkpoint.pending_root.is_some() {
            let drained = drain_replay_rows(
                &mut store,
                &mut head,
                &mut checkpoint,
                &context,
                &policy,
                &path,
            )?;
            made_progress |= drained > 0;
        }
    }

    let replay_end = replay_end(&policy, head.target_extent);
    let normalizer_settled = !checkpoint.acquisition.normalizer_has_pending_events();
    let pending_rows = checkpoint.pending_root.is_some();
    let waiting_for_append = !pending_rows
        && checkpoint.acquisition.committed_offset >= replay_end
        && !normalizer_settled;
    if !pending_rows
        && checkpoint.acquisition.committed_offset >= replay_end
        && normalizer_settled
        && checkpoint.acquisition.status != "incomplete"
    {
        head.before = None;
        head.replay_settled = true;
        if !head.counts_gapped && head.framed {
            head.logical.qualify_narratives(head.target_extent);
            if !store.checkpoint_due() {
                let updates = head.logical.activate_narratives(&mut store, 8)?;
                apply_updates(&mut store, &mut head, updates)?;
            }
        }
    } else {
        head.before = Some(head.target_extent);
        head.replay_settled = false;
        if waiting_for_append && !head.counts_gapped {
            head.progress = "source_waiting_for_append".into();
        }
    }

    head.progress = if head.counts_gapped {
        if head.progress == "indexing" {
            "source_gap".into()
        } else {
            head.progress.clone()
        }
    } else if head.before.is_some() || pending_rows || head.logical.narrative_pending() {
        if head.progress.starts_with("source_") {
            head.progress.clone()
        } else {
            "indexing".into()
        }
    } else if head.root.is_none() {
        "no_admitted_rows".into()
    } else {
        "provisional".into()
    };
    if !head.counts_gapped
        && checkpoint.acquisition.reason.as_deref() == Some("provider_log_partial_record")
    {
        head.progress = if checkpoint.pending_root.is_some() {
            "indexing".into()
        } else {
            "source_partial_record".into()
        };
    }
    if !head.counts_gapped
        && checkpoint.pending_root.is_none()
        && !checkpoint.acquisition.normalizer_has_pending_events()
        && checkpoint.acquisition.status != "incomplete"
    {
        let admitted_through = checkpoint
            .acquisition
            .committed_offset
            .min(head.target_extent);
        if admitted_through >= head.floor {
            head.admitted_through = Some(
                head.admitted_through
                    .map_or(admitted_through, |previous| previous.max(admitted_through)),
            );
        }
    }
    save_head_checkpoint(
        snapshot,
        &mut store,
        &mut head,
        &checkpoint,
        &policy,
        extent,
        &modified,
    )?;
    Ok(head.progress != "source_partial_record"
        && head.progress != "source_waiting_for_append"
        && (head.before.is_some()
            || checkpoint.pending_root.is_some()
            || head.logical.narrative_pending()
            || (extent > head.target_extent && made_progress)))
}

fn new_replay_checkpoint(
    _snapshot: &AgentArchiveCaptureSnapshot,
    context: &super::ConversationArchiveContext,
    policy: &super::chat_read::SourcePolicy,
    extent: u64,
    path: &str,
) -> io::Result<ReplayCheckpoint> {
    let cursor = policy.unknown_before.min(replay_end(policy, extent));
    if cursor < replay_end(policy, extent) && !policy.framed_starts.contains(&cursor) {
        return Err(io::Error::other("source replay prefix is not framed"));
    }
    let source_key = context
        .provider_source_key
        .as_deref()
        .ok_or_else(|| io::Error::other("source replay key absent"))?;
    let acquisition = crate::commands::provider_log_acquisition::source_replay_state(
        Path::new(path),
        source_key,
        &policy.identity,
        cursor,
    )?;
    Ok(ReplayCheckpoint {
        acquisition,
        anchor_floor: cursor,
        pending_root: None,
        pending_before: None,
        pending_count: 0,
    })
}

fn replay_end(policy: &super::chat_read::SourcePolicy, extent: u64) -> u64 {
    policy.open_disabled.unwrap_or(extent).min(extent)
}

fn replay_boundaries_framed(
    snapshot: &AgentArchiveCaptureSnapshot,
    policy: &super::chat_read::SourcePolicy,
    epoch: &str,
    extent: u64,
    native_bytes_read: &mut u64,
) -> io::Result<bool> {
    let mut cursor = policy.unknown_before.min(extent);
    if cursor < replay_end(policy, extent) && !policy.framed_starts.contains(&cursor) {
        return Ok(false);
    }
    for span in &policy.disabled {
        if span.end <= cursor || span.start >= replay_end(policy, extent) {
            continue;
        }
        let start = span.start.min(replay_end(policy, extent));
        if start > cursor && !framed_extent_counted(snapshot, epoch, start, native_bytes_read)? {
            return Ok(false);
        }
        if span.end <= replay_end(policy, extent) {
            if !policy.framed_starts.contains(&span.end) {
                return Ok(false);
            }
            cursor = span.end;
        } else {
            return Ok(true);
        }
    }
    if let Some(open_disabled) = policy.open_disabled.filter(|end| *end <= extent) {
        if cursor < open_disabled
            && !framed_extent_counted(snapshot, epoch, open_disabled, native_bytes_read)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

struct ReplayAdmissionContext<'a> {
    snapshot: &'a AgentArchiveCaptureSnapshot,
    context: &'a super::ConversationArchiveContext,
    policy: &'a super::chat_read::SourcePolicy,
    path: &'a str,
}

// Keep the exact owner/head/checkpoint inputs visible at the admission boundary.
fn queue_replay_events(
    store: &mut Store,
    head: &mut SourceHead,
    checkpoint: &mut ReplayCheckpoint,
    admission_context: ReplayAdmissionContext<'_>,
    events: Vec<AgentChatEvent>,
) -> io::Result<()> {
    let ReplayAdmissionContext {
        snapshot,
        context,
        policy,
        path,
    } = admission_context;
    for mut event in events {
        // Shared acquisition minted this ref from the framed physical row;
        // promote it before strict native admission checks its scope and coordinates.
        if let Some(source_ref) = event.metadata["chat_source_ref"].as_str() {
            event.id = source_ref.to_owned();
        }
        event.metadata["provider_log"] = serde_json::json!(true);
        event.metadata["log_path"] = serde_json::json!(snapshot
            .log_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()));
        event.metadata["provider_session_id"] = serde_json::json!(snapshot
            .resume_session
            .as_ref()
            .or(snapshot.fresh_provider_session_id.as_ref()));
        let coordinates = event.metadata["chat_source_start"]
            .as_u64()
            .zip(event.metadata["chat_source_end"].as_u64());
        let admission = super::chat_logical_index::Admission {
            context,
            conversation: None,
            epoch: &head.epoch,
            admission: &head.admission,
            path,
            watermark: head.target_extent,
            sequence_trusted: false,
        };
        let Some((start, end)) = coordinates else {
            head.counts_gapped = true;
            continue;
        };
        if !policy.admits(start, end)
            || end > head.target_extent
            || !admission.native(&event)
            || !admission.source_owned(&event)
        {
            head.counts_gapped = true;
            continue;
        }
        event.metadata["chat_provisional"] = serde_json::json!(true);
        event.metadata["chat_identity_resolution"] = serde_json::json!("unresolved");
        let identity = crate::providers::chat_transcript::codex_display_identity_sha256(&event);
        if let Some(identity) = identity {
            event.metadata[crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256] =
                serde_json::json!(identity);
        } else if let Some(metadata) = event.metadata.as_object_mut() {
            metadata.remove(crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256);
        }
        event.metadata["chat_source_admission"] = serde_json::json!(head.admission);
        let mut bounded = super::chat_read::header(&event);
        let body_required = event
            .text
            .as_ref()
            .is_some_and(|text| text.len() > super::chat_read::PREVIEW_BYTES)
            || bounded.metadata["chat_tool_input_truncated"] == true
            || event.metadata["tool_input_text"]
                .as_str()
                .is_some_and(|text| text.len() > super::chat_read::PREVIEW_BYTES)
            || event.metadata["text_artifact_refs"]
                .as_array()
                .is_some_and(|references| !references.is_empty())
            || event.metadata["archive_record"]["artifact_refs"]
                .as_array()
                .is_some_and(|references| !references.is_empty());
        bounded.metadata["chat_provisional"] = serde_json::json!(true);
        bounded.metadata["chat_source_admission"] = serde_json::json!(head.admission);
        let row_key = key(&bounded)?;
        let queue_key = replay_pending_key(&row_key)?;
        let pending = ReplayPendingRow {
            key: row_key,
            event: bounded,
            body_required,
        };
        let reference = store.put(&pending)?;
        checkpoint.pending_root =
            Some(store.insert(&checkpoint.pending_root, &queue_key, &reference)?);
        checkpoint.pending_count = checkpoint.pending_count.saturating_add(1);
    }
    Ok(())
}

fn drain_replay_rows(
    store: &mut Store,
    head: &mut SourceHead,
    checkpoint: &mut ReplayCheckpoint,
    context: &super::ConversationArchiveContext,
    policy: &super::chat_read::SourcePolicy,
    path: &str,
) -> io::Result<usize> {
    let Some(root) = checkpoint.pending_root.clone() else {
        return Ok(0);
    };
    let entries = store.page(
        &Some(root),
        checkpoint.pending_before.as_deref(),
        PAGE_ROWS + 1,
    )?;
    if entries.is_empty() {
        checkpoint.pending_root = None;
        checkpoint.pending_before = None;
        checkpoint.pending_count = 0;
        return Ok(0);
    }
    let has_more = entries.len() > PAGE_ROWS;
    let selected = entries.into_iter().take(PAGE_ROWS).collect::<Vec<_>>();
    let mut admitted = 0usize;
    for (queue_key, reference) in &selected {
        let pending: ReplayPendingRow = store.read(reference)?;
        if replay_pending_key(&pending.key)? != *queue_key {
            head.counts_gapped = true;
            continue;
        }
        let event = pending.event;
        let coordinates = event.metadata["chat_source_start"]
            .as_u64()
            .zip(event.metadata["chat_source_end"].as_u64());
        let Some((start, end)) = coordinates else {
            head.counts_gapped = true;
            continue;
        };
        let mut evidence = event.clone();
        evidence.metadata["log_path"] = serde_json::json!(path);
        evidence.metadata["provider_log"] = serde_json::json!(true);
        let admission = super::chat_logical_index::Admission {
            context,
            conversation: None,
            epoch: &head.epoch,
            admission: &head.admission,
            path,
            watermark: head.target_extent,
            sequence_trusted: false,
        };
        if end > head.target_extent
            || !policy.admits(start, end)
            || !admission.native(&evidence)
            || evidence.metadata["chat_source_admission"].as_str() != Some(head.admission.as_str())
            || !admission.source_owned(&evidence)
        {
            head.counts_gapped = true;
            continue;
        }
        let row_key = pending.key;
        let row = SourceRow {
            agent_id: context.agent_id.clone(),
            admission: head.admission.clone(),
            key: row_key.clone(),
            event,
            relation: None,
            removed_display_ids: Vec::new(),
            body_required: pending.body_required,
        };
        let source_row_ref = store.put(&row)?;
        head.root = Some(store.insert(&head.root, &row_key, &source_row_ref)?);
        if row.event.kind == AgentChatEventKind::Message && row.event.role.is_some() {
            if !admission.owned(&evidence) {
                head.counts_gapped = true;
                continue;
            }
            let updates = head
                .logical
                .observe(store, &admission, &evidence, &row_key)?;
            apply_updates(store, head, updates)?;
        }
        admitted += 1;
        head.replay_rows_admitted = head.replay_rows_admitted.saturating_add(1);
    }
    checkpoint.pending_count = checkpoint.pending_count.saturating_sub(selected.len());
    if has_more {
        checkpoint.pending_before = selected.last().map(|(key, _)| key.clone());
    } else {
        checkpoint.pending_root = None;
        checkpoint.pending_before = None;
        checkpoint.pending_count = 0;
    }
    Ok(admitted)
}

fn save_head_checkpoint(
    snapshot: &AgentArchiveCaptureSnapshot,
    store: &mut Store,
    head: &mut SourceHead,
    checkpoint: &ReplayCheckpoint,
    policy: &super::chat_read::SourcePolicy,
    extent_at_start: u64,
    modified_at_start: &str,
) -> io::Result<()> {
    let (epoch, admission, current_extent, current_modified) = source_scope(snapshot)?;
    let current_policy =
        super::chat_read::source_policy(&conversation_archive_context_from_snapshot(snapshot))?;
    if epoch != head.epoch
        || admission != head.admission
        || !policy.same_admission(&current_policy)
        || current_extent < head.target_extent
        || current_extent < extent_at_start
        || (current_extent == head.target_extent && current_modified.as_str() != modified_at_start)
    {
        return Ok(());
    }
    head.replay_state = Some(save_replay_checkpoint(store, checkpoint)?);
    publish(store, head)
}

fn publish(store: &mut Store, head: &SourceHead) -> io::Result<()> {
    let reference = store.put(head)?;
    // Rows and seek roots are immutable and durable before the pointer changes.
    write_json_atomic(
        &locations(&head.agent_id)?
            .0
            .with_file_name("chat-source-head.json"),
        &Some(reference),
    )
}

pub(super) fn framed_extent(
    snapshot: &AgentArchiveCaptureSnapshot,
    epoch: &str,
    extent: u64,
) -> io::Result<bool> {
    let mut native_bytes_read = 0;
    framed_extent_counted(snapshot, epoch, extent, &mut native_bytes_read)
}

fn framed_extent_counted(
    snapshot: &AgentArchiveCaptureSnapshot,
    epoch: &str,
    extent: u64,
    native_bytes_read: &mut u64,
) -> io::Result<bool> {
    use std::io::{Seek, SeekFrom};
    let path = snapshot
        .log_path
        .as_ref()
        .ok_or_else(|| io::Error::other("missing source path"))?;
    let mut file = File::open(path)?;
    let identity = crate::commands::provider_log_acquisition::native_file_identity(&file)?;
    if super::chat_read_store::digest(&serde_json::to_vec(&identity).map_err(io::Error::other)?)
        != epoch
    {
        return Err(io::Error::other("source framing epoch changed"));
    }
    if extent == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(extent - 1))?;
    let mut byte = [0];
    file.read_exact(&mut byte)?;
    *native_bytes_read = (*native_bytes_read).saturating_add(byte.len() as u64);
    Ok(byte[0] == b'\n')
}

fn apply_updates(
    store: &mut Store,
    head: &mut SourceHead,
    updates: Vec<(String, Option<super::chat_logical_index::Relation>)>,
) -> io::Result<()> {
    for (key, relation) in updates {
        let Some(reference) = store.get(&head.root, &key)? else {
            continue;
        };
        let mut row: SourceRow = store.read(&reference)?;
        if row.agent_id != head.agent_id || row.admission != head.admission {
            return Err(io::Error::other("foreign source logical row"));
        }
        if row.relation == relation {
            continue;
        }
        if let Some(old) = &row.relation {
            if relation.as_ref().is_none_or(|new| new.id != old.id)
                && !row.removed_display_ids.contains(&old.id)
            {
                head.invalidations = head.invalidations.saturating_add(1);
                row.removed_display_ids.push(old.id.clone());
                row.removed_display_ids.truncate(4);
            }
        }
        row.relation = relation;
        let reference = store.put(&row)?;
        head.root = Some(store.insert(&head.root, &key, &reference)?);
    }
    Ok(())
}

fn coverage_complete(head: &SourceHead, extent: u64) -> bool {
    head.logical_version >= SOURCE_INDEX_VERSION
        && extent == head.target_extent
        && head.before.is_none()
        && head.framed
        && head.replay_settled
        && !head.counts_gapped
        && head.logical.narratives_completed(head.target_extent)
        && !head.logical.narrative_pending()
}

/// Read only indexed headers under today's independently validated admission.
/// Cursor heads can continue after backfill, but never across source/privacy
/// changes. The caller shares its object/byte budget with the canonical reader.
pub(super) fn page(
    snapshot: &AgentArchiveCaptureSnapshot,
    store: &mut Store,
    reference: &str,
    cursor: Option<&str>,
    previous: Option<&str>,
) -> io::Result<SourcePage> {
    let (head, extent) = admitted_head(snapshot, store, reference)?;
    let counts_complete = coverage_complete(&head, extent);
    let mut reset = previous
        .filter(|r| valid_ref(r))
        .and_then(|r| store.read::<SourceHead>(r).ok())
        .is_some_and(|old| {
            old.generation == head.generation && old.invalidations != head.invalidations
        });
    let mut cursor_frontier = None;
    let before = if let Some(cursor) = cursor {
        let (pinned, before) = cursor
            .strip_prefix("source:")
            .and_then(|cursor| cursor.split_once(':'))
            .ok_or_else(|| io::Error::other("invalid source page cursor"))?;
        cursor_frontier = Some(
            key_position(before).ok_or_else(|| io::Error::other("invalid source page frontier"))?,
        );
        let pinned: SourceHead = store.read(pinned)?;
        if pinned.agent_id != head.agent_id
            || pinned.generation != head.generation
            || pinned.admission != head.admission
        {
            reset = true;
            cursor_frontier = None;
            None
        } else {
            reset |= pinned.invalidations != head.invalidations;
            let (offset, _) = cursor_frontier.expect("the cursor frontier was parsed above");
            if offset < head.floor || offset > head.target_extent {
                reset = true;
                cursor_frontier = None;
                None
            } else {
                Some(before)
            }
        }
    } else {
        None
    };
    let range_covered = if cursor.is_none() {
        counts_complete
    } else {
        cursor_frontier.is_some_and(|(offset, _)| {
            offset >= head.floor
                && offset <= head.target_extent
                && (counts_complete
                    || head
                        .admitted_through
                        .is_some_and(|admitted_through| offset <= admitted_through))
        })
    };
    let keys = store.page(&head.root, before, PAGE_ROWS + 1)?;
    let next_key = if keys.len() > PAGE_ROWS {
        keys.get(PAGE_ROWS - 1).map(|(key, _)| key.as_str())
    } else if head.before.is_some() {
        keys.last().map(|(key, _)| key.as_str()).or(before)
    } else {
        None
    };
    let next = next_key.map(|before| format!("source:{reference}:{before}"));
    let mut events = Vec::new();
    for (key, row_ref) in keys.into_iter().take(PAGE_ROWS) {
        let mut row: SourceRow = store.read(&row_ref)?;
        if row.agent_id != head.agent_id
            || row.admission != head.admission
            || row.key != key
            || row.event.session_id != head.agent_id
            || row.event.metadata["chat_source_epoch"].as_str() != Some(head.epoch.as_str())
            || row.event.metadata["chat_source_ref"].as_str() != Some(row.event.id.as_str())
        {
            return Err(io::Error::other("foreign source row"));
        }
        row.event.metadata["chat_page_key"] = serde_json::json!(key);
        let partial_relation_watermark = head
            .logical
            .completed_watermark()
            .filter(|watermark| *watermark <= extent && *watermark <= head.target_extent);
        let partial_relation_scope_valid = !counts_complete
            && cursor.is_some()
            && range_covered
            && head.logical_version >= SOURCE_INDEX_VERSION
            && head.logical.admits(&head.admission)
            && partial_relation_watermark.is_some();
        let partial_relation_valid = row
            .relation
            .as_ref()
            .zip(partial_relation_watermark)
            .is_some_and(|(relation, watermark)| {
                relation.members.len() == 2
                    && relation.members[0] != relation.members[1]
                    && relation.members.contains(&relation.id)
                    && relation.members.contains(&row.event.id)
                    && relation.members.iter().all(|member| {
                        source_id_parts(&head.agent_id, member).is_some_and(|(epoch, _, offset)| {
                            epoch == head.epoch && offset < watermark
                        })
                    })
                    && row.event.metadata["chat_source_start"]
                        .as_u64()
                        .is_some_and(|start| start < watermark)
                    && row.event.metadata["chat_source_end"]
                        .as_u64()
                        .is_some_and(|end| end <= watermark && end <= extent)
            });
        if !counts_complete
            && row.relation.is_some()
            && !(partial_relation_scope_valid && partial_relation_valid)
        {
            reset = true;
        }
        let relation = row.relation.filter(|_| {
            counts_complete || (partial_relation_scope_valid && partial_relation_valid)
        });
        if let Some(relation) = relation {
            row.event.metadata["chat_display_physical_id"] = serde_json::json!(row.event.id);
            row.event.id = relation.id;
            row.event.metadata["chat_display_member_ids"] = serde_json::json!(relation.members);
        }
        if !row.removed_display_ids.is_empty() {
            row.event.metadata["chat_display_removed_ids"] =
                serde_json::json!(row.removed_display_ids);
        }
        events.push(row.event);
    }
    events.reverse();
    Ok(SourcePage {
        reference: reference.to_owned(),
        generation: head.generation,
        epoch: head.epoch,
        admission: head.admission,
        target_extent: head.target_extent,
        progress: head.progress,
        counts_complete,
        events,
        next_before: next,
        cursor_frontier,
        range_covered,
        reset,
    })
}

fn admitted_head(
    snapshot: &AgentArchiveCaptureSnapshot,
    store: &mut Store,
    reference: &str,
) -> io::Result<(SourceHead, u64)> {
    let context = conversation_archive_context_from_snapshot(snapshot);
    let (epoch, admission, extent, modified) = source_scope(snapshot)?;
    let head: SourceHead = store.read(reference)?;
    if head.agent_id != context.agent_id
        || head.source_key != context.provider_source_key
        || head.epoch != epoch
        || head.admission != admission
        || extent < head.target_extent
        || (extent == head.target_extent && modified != head.modified)
    {
        return Err(io::Error::other("source projection scope changed"));
    }
    Ok((head, extent))
}

pub(super) fn scope(
    snapshot: &AgentArchiveCaptureSnapshot,
    store: &mut Store,
    reference: &str,
) -> io::Result<(String, String, String)> {
    let (head, _) = admitted_head(snapshot, store, reference)?;
    Ok((head.generation, head.epoch, head.progress))
}

/// Find whether older provisional observations remain without decoding a page.
/// The recent seed already independently validated this exact admission.
pub(super) fn continuation(
    snapshot: &AgentArchiveCaptureSnapshot,
    store: &mut Store,
    reference: &str,
    checkpoint: &SeedCheckpoint,
    before: Option<&AgentChatEvent>,
) -> io::Result<Option<String>> {
    let context = conversation_archive_context_from_snapshot(snapshot);
    let head: SourceHead = store.read(reference)?;
    if head.agent_id != context.agent_id
        || head.source_key != context.provider_source_key
        || head.epoch != checkpoint.epoch
        || head.admission != checkpoint.admission
        || checkpoint.extent < head.target_extent
    {
        return Err(io::Error::other("source continuation scope changed"));
    }
    let Some(before) = before.map(key).transpose()? else {
        return Ok(None);
    };
    Ok(
        (!store.page(&head.root, Some(&before), 1)?.is_empty() || head.before.is_some())
            .then(|| format!("source:{reference}:{before}")),
    )
}
