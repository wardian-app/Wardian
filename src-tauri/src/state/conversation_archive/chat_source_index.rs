//! Independently framed source observations, never a canonical archive seal.
//! The existing background capture owner publishes recent rows before walking
//! older intervals. A restart resumes the last immutable checkpoint.
use std::fs::File;
use std::io::{self, Read};

use serde::{Deserialize, Serialize};
use wardian_core::conversations::write_json_atomic;
use wardian_core::models::chat::AgentChatEvent;

use super::chat_read::{locations, PAGE_ROWS};
use super::chat_read_store::{valid_ref, Store};
use crate::commands::chat::{
    conversation_archive_context_from_snapshot, AgentArchiveCaptureSnapshot,
};
use crate::commands::chat_recent_seed::{read_window, source_scope, SeedCheckpoint};

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
    invalidations: u64,
    target_extent: u64,
    modified: String,
    floor: u64,
    before: Option<u64>,
    progress: String,
    #[serde(default)]
    counts_gapped: bool,
    #[serde(default)]
    framed: bool,
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
}

pub(super) struct SourcePage {
    pub(super) generation: String,
    pub(super) epoch: String,
    pub(super) progress: String,
    pub(super) events: Vec<AgentChatEvent>,
    pub(super) next_before: Option<String>,
    pub(super) reset: bool,
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

/// One bounded reverse checkpoint, called only by the coalesced capture owner
/// after privacy policy commit and before full canonical archive maintenance.
pub(crate) fn advance(snapshot: &AgentArchiveCaptureSnapshot) -> io::Result<bool> {
    let context = conversation_archive_context_from_snapshot(snapshot);
    let Ok((epoch, admission, extent, modified)) = source_scope(snapshot) else {
        return Ok(false);
    };
    let mut store = Store::writer(&locations(&context.agent_id)?.1);
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
            invalidations: 0,
            target_extent: extent,
            modified: modified.clone(),
            floor: 0,
            before: Some(extent),
            progress: "indexing".into(),
            counts_gapped: false,
            framed: false,
        });
    let policy = super::chat_read::source_policy(&context)?;
    head.framed = framed_boundary(snapshot, &head)?;
    if head.before.is_none() {
        if head.framed
            && !head.counts_gapped
            && policy.unknown_before == 0
            && policy.disabled.is_empty()
            && policy.open_disabled.is_none()
        {
            head.logical.qualify_narratives(head.target_extent);
        }
        if head.logical.narrative_pending() {
            let updates = head.logical.activate_narratives(&mut store, 8)?;
            apply_updates(&mut store, &mut head, updates)?;
            head.progress = if head.logical.narrative_pending() {
                "indexing"
            } else {
                "provisional"
            }
            .into();
            let pending = head.logical.narrative_pending() || extent > head.target_extent;
            publish(&mut store, &head)?;
            return Ok(pending);
        }
    }
    if head.before.is_none() {
        if extent == head.target_extent {
            return Ok(false);
        }
        // Finish a fixed interval before admitting the next append interval.
        // Continuous output cannot restart and starve the older-history walk.
        // A prior unframed suffix was never counted. Revisit it through a
        // bounded reverse walk after completion; physical tokens deduplicate
        // already-counted rows without moving any native coordinate.
        head.floor = if head.framed { head.target_extent } else { 0 };
        head.target_extent = extent;
        head.modified = modified;
        head.before = Some(extent);
        head.framed = framed_boundary(snapshot, &head)?;
    }
    let seed = read_window(snapshot, None, head.before)?;
    let Some(checkpoint) = seed.checkpoint else {
        return Ok(true);
    };
    if checkpoint.epoch != head.epoch
        || checkpoint.admission != head.admission
        || checkpoint.extent < head.target_extent
    {
        return Ok(true);
    }
    let path = std::fs::canonicalize(
        snapshot
            .log_path
            .as_ref()
            .ok_or_else(|| io::Error::other("missing source path"))?,
    )?
    .to_string_lossy()
    .to_string();
    head.logical.defer_narratives();
    head.counts_gapped |= seed.progress == "oversized_record";
    let mut resume_before = None;
    let mut processed_before = head.before.unwrap_or(head.target_extent);
    for event in seed.events.into_iter().rev() {
        if store.checkpoint_due() {
            // A payload end excludes its newline. Resume at the last
            // committed start so the next older record remains complete.
            resume_before = Some(processed_before);
            break;
        }
        processed_before = event.metadata["chat_source_start"]
            .as_u64()
            .unwrap_or(processed_before);
        let key = key(&event)?;
        let evidence = event.clone();
        let row = SourceRow {
            agent_id: context.agent_id.clone(),
            admission: head.admission.clone(),
            key: key.clone(),
            event,
            relation: None,
            removed_display_ids: Vec::new(),
        };
        let reference = store.put(&row)?;
        head.root = Some(store.insert(&head.root, &key, &reference)?);
        if evidence.metadata["chat_source_start"]
            .as_u64()
            .zip(evidence.metadata["chat_source_end"].as_u64())
            .is_some_and(|(start, end)| policy.admits(start, end))
        {
            let updates = head.logical.observe(
                &mut store,
                &super::chat_logical_index::Admission {
                    context: &context,
                    conversation: None,
                    epoch: &head.epoch,
                    admission: &head.admission,
                    path: &path,
                    watermark: head.target_extent,
                    sequence_trusted: false,
                },
                &evidence,
                &key,
            )?;
            apply_updates(&mut store, &mut head, updates)?;
        }
    }
    head.before = resume_before
        .or(checkpoint.before)
        .filter(|before| *before > head.floor);
    if head.before.is_none()
        && head.framed
        && !head.counts_gapped
        && policy.unknown_before == 0
        && policy.disabled.is_empty()
        && policy.open_disabled.is_none()
    {
        head.logical.qualify_narratives(head.target_extent);
        // Leave writer headroom after row admission; the persisted queue can
        // always continue in the next fixed checkpoint after restart.
        if !store.checkpoint_due() {
            let updates = head.logical.activate_narratives(&mut store, 8)?;
            apply_updates(&mut store, &mut head, updates)?;
        }
    }
    head.progress = if matches!(
        seed.progress.as_str(),
        "oversized_record" | "no_admitted_rows"
    ) {
        seed.progress
    } else if head.before.is_some() || head.logical.narrative_pending() {
        "indexing".into()
    } else if head.root.is_none() {
        "no_admitted_rows".into()
    } else {
        "provisional".into()
    };
    let pending =
        head.before.is_some() || head.logical.narrative_pending() || extent > head.target_extent;
    publish(&mut store, &head)?;
    Ok(pending)
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

fn framed_boundary(snapshot: &AgentArchiveCaptureSnapshot, head: &SourceHead) -> io::Result<bool> {
    use std::io::{Seek, SeekFrom};
    let path = snapshot
        .log_path
        .as_ref()
        .ok_or_else(|| io::Error::other("missing source path"))?;
    let mut file = File::open(path)?;
    let identity = crate::commands::provider_log_acquisition::native_file_identity(&file)?;
    if super::chat_read_store::digest(&serde_json::to_vec(&identity).map_err(io::Error::other)?)
        != head.epoch
    {
        return Err(io::Error::other("source framing epoch changed"));
    }
    if head.target_extent == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(head.target_extent - 1))?;
    let mut byte = [0];
    file.read_exact(&mut byte)?;
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
    let counts_complete = extent == head.target_extent
        && head.before.is_none()
        && head.framed
        && !head.counts_gapped
        && head.logical.narratives_completed(head.target_extent);
    let mut reset = previous
        .filter(|r| valid_ref(r))
        .and_then(|r| store.read::<SourceHead>(r).ok())
        .is_some_and(|old| {
            old.generation == head.generation && old.invalidations != head.invalidations
        });
    let before = if let Some(cursor) = cursor {
        let (pinned, before) = cursor
            .strip_prefix("source:")
            .and_then(|cursor| cursor.split_once(':'))
            .ok_or_else(|| io::Error::other("invalid source page cursor"))?;
        let pinned: SourceHead = store.read(pinned)?;
        if pinned.agent_id != head.agent_id
            || pinned.generation != head.generation
            || pinned.admission != head.admission
        {
            reset = true;
            None
        } else {
            reset |= pinned.invalidations != head.invalidations;
            Some(before)
        }
    } else {
        None
    };
    let keys = store.page(&head.root, before, PAGE_ROWS + 1)?;
    let next = if keys.len() > PAGE_ROWS || head.before.is_some() {
        let before = keys
            .get(PAGE_ROWS - 1)
            .or_else(|| keys.last())
            .map(|(key, _)| key.as_str())
            .or(before)
            .unwrap_or("~");
        Some(format!("source:{reference}:{before}"))
    } else {
        None
    };
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
        if !counts_complete && row.relation.is_some() {
            reset = true;
        }
        if let Some(relation) = row.relation.filter(|_| counts_complete) {
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
        generation: head.generation,
        epoch: head.epoch,
        progress: head.progress,
        events,
        next_before: next,
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
    let before = before.map(key).transpose()?.unwrap_or_else(|| "~".into());
    Ok(
        (!store.page(&head.root, Some(&before), 1)?.is_empty() || head.before.is_some())
            .then(|| format!("source:{reference}:{before}")),
    )
}
