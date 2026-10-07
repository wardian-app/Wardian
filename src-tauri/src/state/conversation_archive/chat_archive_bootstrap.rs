//! Cold, derived-only admission under the existing capture owner's gates.
//! Every pass reads one bounded JSONL window. Immutable roots and cursors live
//! in the published head, so a restart never mistakes a partial index for ready.
use super::*;
use wardian_core::conversations::{
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationStatus,
};

const WINDOW_BYTES: u64 = 256 * 1024;
const ADMIT_ROWS: usize = 24;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: String,
    identity: crate::commands::provider_log_acquisition::ProviderLogNativeIdentity,
}

impl Stamp {
    fn read(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        Ok(Self {
            length: metadata.len(),
            modified: format!("{:?}", metadata.modified()?),
            identity: crate::commands::provider_log_acquisition::native_file_identity(&file)?,
        })
    }
    fn validate(&self, path: &Path) -> io::Result<()> {
        if Self::read(path)? != *self {
            return Err(io::Error::other("saved chat file changed during bootstrap"));
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Progress {
    #[serde(default)]
    selection_scope: Option<SelectionScope>,
    index: Stamp,
    manifest: Stamp,
    narrative: Stamp,
    events: Stamp,
    narrative_before: u64,
    event_before: u64,
    proofs: Option<String>,
    bodies: Option<String>,
    body_before: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SelectionScope {
    agent_id: String,
    provider: String,
    provider_source_key: Option<String>,
    provider_session_ids: Vec<String>,
}

impl SelectionScope {
    fn new(context: &ConversationArchiveContext) -> Self {
        Self {
            agent_id: context.agent_id.clone(),
            provider: context.provider.clone(),
            provider_source_key: context.provider_source_key.clone(),
            provider_session_ids: context.provider_session_ids.clone(),
        }
    }

    fn matches(&self, context: &ConversationArchiveContext) -> bool {
        self.agent_id == context.agent_id
            && self.provider == context.provider
            && self.provider_source_key == context.provider_source_key
            && self.provider_session_ids == context.provider_session_ids
    }
}

/// Cold checkpoints must retain the complete selection binding across restart.
/// Older unbound checkpoints require reselection; warm candidates keep their
/// existing admission contract.
pub(super) fn scope_matches(head: &Head, context: &ConversationArchiveContext) -> bool {
    head.bootstrap.as_ref().is_none_or(|progress| {
        progress
            .selection_scope
            .as_ref()
            .is_some_and(|scope| scope.matches(context))
    })
}

#[derive(Serialize, Deserialize)]
struct Selection {
    scope: String,
    index: Stamp,
    before: u64,
    seen: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Proof {
    generated: bool,
    input_binding: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct BodyJob {
    event_start: u64,
    event_bytes: u64,
    row: Row,
    offset: u64,
}

/// Physical line starts are stable private ordering keys. They are deliberately
/// not narrative sequence numbers (older event envelopes often omit sequence).
/// Oversized individual envelopes fail explicitly instead of allocating a whole
/// archive or silently skipping a row. Artifact bodies keep the existing chunks.
fn window(path: &Path, before: u64) -> io::Result<Vec<(u64, Vec<u8>)>> {
    let mut file = File::open(path)?;
    let start = before.saturating_sub(WINDOW_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (before - start) as usize];
    file.read_exact(&mut bytes)?;
    let mut end = bytes.len();
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    let mut rows = Vec::new();
    while end > 0 && rows.len() < ADMIT_ROWS {
        let line_start = bytes[..end]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1);
        if line_start.is_none() && start > 0 {
            break;
        }
        let line_start = line_start.unwrap_or(0);
        if line_start < end {
            rows.push((start + line_start as u64, bytes[line_start..end].to_vec()));
        }
        end = line_start.saturating_sub(1);
    }
    if rows.is_empty() && before > 0 {
        return Err(io::Error::other(
            "saved chat envelope exceeds bootstrap window",
        ));
    }
    Ok(rows)
}

fn bounded_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    let mut file = File::open(path)?;
    let extent = file.metadata()?.len();
    if extent > WINDOW_BYTES {
        return Err(io::Error::other("oversized saved chat metadata"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(extent + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != extent {
        return Err(io::Error::other("saved chat metadata changed"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn selection_path(context: &ConversationArchiveContext) -> io::Result<PathBuf> {
    Ok(locations(&context.agent_id)?
        .0
        .with_file_name("chat-bootstrap-selection.json"))
}

fn select(
    context: &ConversationArchiveContext,
    store: &mut Store,
) -> io::Result<(Option<Head>, bool)> {
    let index_path = super::super::storage::index_path(&context.agent_id)?;
    let stamp = match Stamp::read(&index_path) {
        Ok(stamp) => stamp,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((None, false)),
        Err(error) => return Err(error),
    };
    let scope = digest(&serde_json::to_vec(&context_scope(context)).map_err(io::Error::other)?);
    let pointer = selection_path(context)?;
    let previous: Option<String> = match bounded_json(&pointer) {
        Ok(previous) => previous,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut selection = if let Some(reference) = previous {
        let old: Selection = store.read(&reference)?;
        if old.scope == scope && old.index == stamp {
            old
        } else {
            Selection {
                scope,
                before: stamp.length,
                index: stamp,
                seen: None,
            }
        }
    } else {
        Selection {
            scope,
            before: stamp.length,
            index: stamp,
            seen: None,
        }
    };
    if selection.before == 0 {
        return Ok((None, false));
    }
    // The latest record for each conversation wins, including a closed record.
    // A persistent seen tree bounds memory even for a long index journal.
    for (offset, bytes) in window(&index_path, selection.before)? {
        if store.checkpoint_due() {
            break;
        }
        let entry: ConversationIndexEntry =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        selection.before = offset;
        let key = digest(entry.conversation_id.as_bytes());
        if store.get(&selection.seen, &key)?.is_some() {
            continue;
        }
        let marker = store.put(&true)?;
        selection.seen = Some(store.insert(&selection.seen, &key, &marker)?);
        if entry.agent_id != context.agent_id
            || entry.provider != context.provider
            || entry.status != ConversationStatus::Open
        {
            continue;
        }
        let directory =
            super::super::storage::conversation_dir(&context.agent_id, &entry.conversation_id)?;
        let manifest_path = directory.join("manifest.json");
        let manifest: ConversationManifest = bounded_json(&manifest_path)?;
        if manifest.agent_id != context.agent_id
            || manifest.conversation_id != entry.conversation_id
            || manifest.provider != context.provider
            || manifest.status != ConversationStatus::Open
            || manifest.effective_logging != ConversationLoggingSetting::Enabled
            || manifest.provider_source_key != context.provider_source_key
            || !context.provider_session_ids.iter().all(|id| {
                manifest.provider_session_ids.contains(id)
                    && entry.provider_session_ids.contains(id)
            })
        {
            continue;
        }
        let narrative = Stamp::read(&directory.join("conversation.jsonl"))?;
        let events = Stamp::read(&directory.join("events.jsonl"))?;
        selection.index.validate(&index_path)?;
        let progress = Progress {
            selection_scope: Some(SelectionScope::new(context)),
            index: selection.index,
            manifest: Stamp::read(&manifest_path)?,
            narrative_before: narrative.length,
            event_before: events.length,
            narrative,
            events,
            proofs: None,
            bodies: None,
            body_before: None,
        };
        return Ok((
            Some(Head {
                agent_id: context.agent_id.clone(),
                conversation_id: entry.conversation_id,
                source_key: context.provider_source_key.clone(),
                generation: uuid::Uuid::new_v4().to_string(),
                source_epoch: None,
                root: None,
                identities: None,
                logical: Default::default(),
                committed_output_bytes: 0,
                capture_stamp: "saved-archive".into(),
                progress: "indexing".into(),
                parent: None,
                changes: Vec::new(),
                recent_start: 0,
                row_count: 0,
                bootstrap: Some(progress),
            }),
            true,
        ));
    }
    selection.index.validate(&index_path)?;
    let pending = selection.before > 0;
    let reference = store.put(&selection)?;
    write_json_atomic(&pointer, &Some(reference))?;
    Ok((None, pending))
}

fn context_scope(
    context: &ConversationArchiveContext,
) -> (&str, &str, &Option<String>, &Vec<String>) {
    (
        &context.agent_id,
        &context.provider,
        &context.provider_source_key,
        &context.provider_session_ids,
    )
}

fn candidate(context: &ConversationArchiveContext, conversation_id: &str) -> Candidate {
    Candidate {
        context: context.clone(),
        conversation_id: conversation_id.into(),
        events: Vec::new(),
        committed_output_bytes: 0,
        verified_ids: Default::default(),
        generated_ids: Default::default(),
        generated_input_bindings: Default::default(),
        source_epoch: None,
    }
}

/// A stored display link is not a native-coordinate proof. Preserve canonical
/// IDs while leaving these cold legacy links unresolved until an owner commits
/// a positively verified candidate. Never match or alias by text.
fn unresolved(event: &mut AgentChatEvent) {
    if event.metadata["chat_source_ref"].is_string() {
        event.metadata["chat_legacy_identity_ineligible"] = json!(true);
    }
    if let Some(metadata) = event.metadata.as_object_mut() {
        for key in [
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
            "chat_source_admission",
            "legacy_event_ids",
            "request_root_id",
        ] {
            metadata.remove(key);
        }
        metadata.insert("chat_identity_resolution".into(), json!("unresolved"));
    }
}

pub(super) fn advance(context: &ConversationArchiveContext) -> io::Result<bool> {
    let (head_path, objects) = locations(&context.agent_id)?;
    let mut store = Store::bounded_writer(&objects);
    let previous = head(&context.agent_id)?;
    let mut published = match &previous {
        Some(reference) => store.read::<Head>(reference)?,
        None => match select(context, &mut store)? {
            (Some(head), _) => head,
            (None, pending) => return Ok(pending),
        },
    };
    if published.agent_id != context.agent_id
        || published.source_key != context.provider_source_key
        || !scope_matches(&published, context)
    {
        retire(&context.agent_id)?;
        return Ok(true);
    }
    let Some(mut progress) = published.bootstrap.take() else {
        return Ok(false);
    };
    let directory =
        super::super::storage::conversation_dir(&context.agent_id, &published.conversation_id)?;
    let index_path = super::super::storage::index_path(&context.agent_id)?;
    let narrative_path = directory.join("conversation.jsonl");
    let event_path = directory.join("events.jsonl");
    if progress
        .index
        .validate(&index_path)
        .and_then(|()| progress.manifest.validate(&directory.join("manifest.json")))
        .and_then(|()| progress.narrative.validate(&narrative_path))
        .and_then(|()| progress.events.validate(&event_path))
        .is_err()
    {
        retire(&context.agent_id)?;
        return Ok(true);
    }
    let mut changes = HashMap::new();
    let source_proof = super::super::chat_logical_index::source_proof(context)?;
    published.source_epoch = source_proof.as_ref().map(|source| source.epoch.clone());
    published.logical.defer_narratives();
    if progress.narrative_before > 0 {
        for (offset, bytes) in window(&narrative_path, progress.narrative_before)? {
            // Leave room for one maximum-size provenance record and the head.
            if store.checkpoint_due() {
                break;
            }
            let record: ConversationNarrativeRecord =
                serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if record.event_refs.len() > 16 {
                return Err(io::Error::other("oversized saved chat provenance"));
            }
            let bindings =
                generated_input_bindings(std::slice::from_ref(&record), &published.conversation_id);
            for id in &record.event_refs {
                if id.len() > 256 {
                    return Err(io::Error::other("oversized saved chat identity"));
                }
                let key = digest(id.as_bytes());
                if store.get(&progress.proofs, &key)?.is_none() {
                    let proof = Proof {
                        generated: id
                            == &format!("generated:{}:{}", published.conversation_id, record.seq),
                        input_binding: bindings.get(id).cloned(),
                    };
                    let reference = store.put(&proof)?;
                    progress.proofs = Some(store.insert(&progress.proofs, &key, &reference)?);
                }
            }
            progress.narrative_before = offset;
        }
    }
    if progress.event_before > 0 {
        let mut candidate = candidate(context, &published.conversation_id);
        for (offset, bytes) in window(&event_path, progress.event_before)? {
            if store.checkpoint_due() {
                break;
            }
            let mut event: AgentChatEvent =
                serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if event.session_id != context.agent_id || event.id.len() > 256 {
                return Err(io::Error::other("foreign saved chat event"));
            }
            let proof = store
                .get(&progress.proofs, &digest(event.id.as_bytes()))?
                .map(|reference| store.read::<Proof>(&reference))
                .transpose()?;
            // Publish recent proved rows while older provenance is walking.
            // An unproved row waits for its proof instead of being discarded.
            if proof.is_none() && progress.narrative_before > 0 {
                break;
            }
            progress.event_before = offset;
            if proof.is_none() {
                continue;
            }
            if event.metadata["generated"] == true {
                let Some(proof) = proof.filter(|proof| proof.generated) else {
                    continue;
                };
                candidate.generated_ids.insert(event.id.clone());
                if let Some(binding) = proof.input_binding {
                    candidate
                        .generated_input_bindings
                        .insert(event.id.clone(), binding);
                }
            }
            if event.provider != context.provider && !is_owned_unknown_input(&candidate, &event) {
                continue;
            }
            // Reverse admission selects the latest envelope for an existing ID.
            if store
                .get(&published.identities, &digest(event.id.as_bytes()))?
                .is_some()
            {
                // Repeated historical envelopes are separate occurrences even
                // when the old physical reader selects only their latest ID.
                // Count them before that selection can hide ambiguity.
                if !event.metadata["chat_source_ref"].is_string() {
                    if let Some(proof) = &source_proof {
                        let updates = published.logical.observe(
                            &mut store,
                            &proof.admission(context, &published.conversation_id),
                            &event,
                            &format!("{offset:020}"),
                        )?;
                        apply_logical_updates(&mut store, &mut published, updates, &mut changes)?;
                    }
                }
                continue;
            }
            let admitted = source_proof.as_ref().is_some_and(|proof| {
                proof
                    .admission(context, &published.conversation_id)
                    .native(&event)
                    && event.metadata["chat_source_start"]
                        .as_u64()
                        .zip(event.metadata["chat_source_end"].as_u64())
                        .is_some_and(|(start, end)| proof.policy.admits(start, end))
            });
            if !admitted {
                unresolved(&mut event);
            }
            let mut row = Row {
                agent_id: context.agent_id.clone(),
                conversation_id: published.conversation_id.clone(),
                key: format!("{offset:020}"),
                event: header(&event),
                body: None,
                relation: None,
                removed_display_ids: Vec::new(),
            };
            row.event.sequence = Some(offset + 1);
            // A missing optional artifact cannot hold every older header at
            // this checkpoint. body_source validates the owned path first.
            let source = match body_source(&candidate, &event) {
                Ok(source) => source,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    row.event.metadata["chat_body_unavailable"] = json!(true);
                    None
                }
                Err(error) => return Err(error),
            };
            if let Some(source) = source {
                row.body = Some(Body {
                    binding: source.binding()?,
                    ..Default::default()
                });
                let job = store.put(&BodyJob {
                    event_start: offset,
                    event_bytes: bytes.len() as u64,
                    row: row.clone(),
                    offset: 0,
                })?;
                progress.bodies = Some(store.insert(&progress.bodies, &row.key, &job)?);
            }
            published.row_count = published.row_count.max(offset as usize + 1);
            let reference = insert_row(&mut store, &mut published, &row)?;
            changes.insert(row.event.id.clone(), reference);
            if let Some(proof) = &source_proof {
                let updates = published.logical.observe(
                    &mut store,
                    &proof.admission(context, &published.conversation_id),
                    &event,
                    &row.key,
                )?;
                apply_logical_updates(&mut store, &mut published, updates, &mut changes)?;
            }
        }
    }
    // One persistent body job, at most four existing 16-KiB chunks per pass.
    if let Some((key, reference)) = store
        .page(&progress.bodies, progress.body_before.as_deref(), 1)?
        .into_iter()
        .next()
    {
        let mut job: BodyJob = store.read(&reference)?;
        let candidate = candidate(context, &published.conversation_id);
        if job.event_bytes > WINDOW_BYTES
            || job.event_start.saturating_add(job.event_bytes) > progress.events.length
        {
            return Err(io::Error::other("invalid saved chat body coordinates"));
        }
        let mut file = File::open(&event_path)?;
        file.seek(SeekFrom::Start(job.event_start))?;
        let mut bytes = vec![0; job.event_bytes as usize];
        file.read_exact(&mut bytes)?;
        let event: AgentChatEvent = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if event.id != job.row.event.id || event.session_id != context.agent_id {
            return Err(io::Error::other("foreign saved chat body"));
        }
        let source = match body_source(&candidate, &event) {
            Ok(Some(source)) => Some(source),
            Ok(None) => return Err(io::Error::other("saved chat body disappeared")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Keep the published prefix and binding; settle only this job.
                job.row.event.metadata["chat_body_unavailable"] = json!(true);
                None
            }
            Err(error) => return Err(error),
        };
        if let Some(mut source) = source {
            let binding = source.binding()?;
            if job
                .row
                .body
                .as_ref()
                .is_none_or(|body| body.binding != binding)
            {
                return Err(io::Error::other("saved chat body changed"));
            }
            for _ in 0..4 {
                let bytes = source.chunk(job.offset)?;
                let length = source.length()?;
                if bytes.is_empty() && job.offset < length {
                    return Err(io::Error::other("saved chat body EOF"));
                }
                let body = job.row.body.as_mut().expect("saved body descriptor");
                let reference = store.put_bytes(&bytes)?;
                body.root =
                    Some(store.insert(&body.root, &format!("{:020}", job.offset), &reference)?);
                job.offset += bytes.len() as u64;
                body.bytes = job.offset;
                body.complete = job.offset == length;
                if body.complete {
                    break;
                }
            }
        }
        let complete = job.row.body.as_ref().is_some_and(|body| body.complete);
        if let Some(reference) = store.get(&published.root, &job.row.key)? {
            let current: Row = store.read(&reference)?;
            job.row.relation = current.relation;
            job.row.removed_display_ids = current.removed_display_ids;
        }
        let reference = insert_row(&mut store, &mut published, &job.row)?;
        changes.insert(job.row.event.id.clone(), reference);
        if complete || job.row.event.metadata["chat_body_unavailable"] == true {
            progress.body_before = Some(key);
        } else {
            let reference = store.put(&job)?;
            progress.bodies = Some(store.insert(&progress.bodies, &key, &reference)?);
        }
    }
    let body_pending = !store
        .page(&progress.bodies, progress.body_before.as_deref(), 1)?
        .is_empty();
    if progress.narrative_before == 0 && progress.event_before == 0 {
        published
            .logical
            .qualify_sequences(source_proof.as_ref().is_some_and(|s| s.sequence_trusted));
        published
            .logical
            .qualify_legacy(source_proof.as_ref().is_some_and(|s| s.complete_prefix));
        if let Some(proof) = source_proof
            .as_ref()
            .filter(|proof| proof.narrative_complete)
        {
            published.logical.qualify_narratives(proof.watermark);
            if !store.checkpoint_due() {
                let updates = published.logical.activate_narratives(&mut store, 2)?;
                apply_logical_updates(&mut store, &mut published, updates, &mut changes)?;
            }
        }
        if !store.checkpoint_due() {
            let updates = published.logical.activate(&mut store, 2)?;
            apply_logical_updates(&mut store, &mut published, updates, &mut changes)?;
        }
    }
    let pending = progress.narrative_before > 0
        || progress.event_before > 0
        || body_pending
        || published.logical.pending()
        || published.logical.narrative_pending();
    // Byte-order keys require an actual newest-window boundary, not len - 80.
    published.recent_start = store
        .page(&published.root, None, PAGE_ROWS)?
        .last()
        .map(|(key, _)| key.parse::<usize>().map_err(io::Error::other))
        .transpose()?
        .unwrap_or(0);
    published.progress = if pending { "indexing" } else { "ready" }.into();
    published.parent = previous;
    if changes.len() > PAGE_ROWS {
        published.parent = None;
    }
    published.changes = changes.into_values().take(PAGE_ROWS).collect();
    published.bootstrap = Some(progress);
    let progress = published.bootstrap.as_ref().expect("bootstrap progress");
    progress.index.validate(&index_path)?;
    progress
        .manifest
        .validate(&directory.join("manifest.json"))?;
    progress.narrative.validate(&narrative_path)?;
    progress.events.validate(&event_path)?;
    let reference = store.put(&published)?;
    // LAST: cursors, provenance roots, rows and body chunks are durable together.
    write_json_atomic(&head_path, &reference)?;
    Ok(pending)
}

pub(super) fn retire(agent_id: &str) -> io::Result<()> {
    let path = locations(agent_id)?.0;
    write_json_atomic(&path, &Option::<String>::None)?;
    write_json_atomic(
        &path.with_file_name("chat-bootstrap-selection.json"),
        &Option::<String>::None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_saved_window_bounds_rows_and_rejects_giant_envelopes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let text = (0..100)
            .map(|i| format!("{{\"i\":{i}}}\n"))
            .collect::<String>();
        std::fs::write(&path, &text).unwrap();
        let rows = window(&path, text.len() as u64).unwrap();
        assert_eq!(rows.len(), ADMIT_ROWS);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&rows[0].1).unwrap()["i"],
            99
        );
        assert!(rows.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() <= WINDOW_BYTES as usize);
        std::fs::write(&path, vec![b'x'; WINDOW_BYTES as usize + 1]).unwrap();
        assert!(window(&path, WINDOW_BYTES + 1)
            .unwrap_err()
            .to_string()
            .contains("exceeds bootstrap window"));
    }

    #[test]
    fn cold_saved_attempted_object_writes_have_a_byte_budget() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::bounded_writer(temp.path());
        let bytes = vec![0; BODY_BYTES];
        for _ in 0..512 {
            store.put_bytes(&bytes).unwrap();
        }
        assert!(store
            .put_bytes(&bytes)
            .unwrap_err()
            .to_string()
            .contains("write budget exhausted"));
    }
}
