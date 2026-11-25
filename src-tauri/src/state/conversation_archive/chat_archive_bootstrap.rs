//! Cold, derived-only admission under the existing capture owner's gates.
//! Every pass reads one bounded JSONL window. Immutable roots and cursors live
//! in the published head, so a restart never mistakes a partial index for ready.
use super::*;
#[cfg(test)]
use wardian_core::conversations::{
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationStatus,
};

#[cfg(test)]
const WINDOW_BYTES: u64 = 256 * 1024;
#[cfg(test)]
const ADMIT_ROWS: usize = 24;
#[cfg(test)]
const BODY_CHUNKS_PER_PASS: usize = 16;
#[cfg(test)]
const BODY_JOBS_PER_PASS: usize = 16;
#[cfg(test)]
const RESERVED_BODY_CHUNKS: usize = 4;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: String,
    identity: crate::commands::provider_log_acquisition::ProviderLogNativeIdentity,
}

impl Stamp {
    #[cfg(test)]
    fn read(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        Ok(Self {
            length: metadata.len(),
            modified: format!("{:?}", metadata.modified()?),
            identity: crate::commands::provider_log_acquisition::native_file_identity(&file)?,
        })
    }
    #[cfg(test)]
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
    #[serde(default)]
    archive_display: Option<super::super::chat_logical_index::Index>,
}

impl Progress {
    #[cfg(test)]
    /// Body/proof indexes are roots; their seek positions and stamps are not.
    pub(super) fn node_roots(&self) -> impl Iterator<Item = &str> {
        [self.proofs.as_deref(), self.bodies.as_deref()]
            .into_iter()
            .flatten()
            .chain(
                self.archive_display
                    .iter()
                    .flat_map(|index| index.node_roots()),
            )
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SelectionScope {
    agent_id: String,
    provider: String,
    provider_source_key: Option<String>,
    provider_session_ids: Vec<String>,
}

impl SelectionScope {
    #[cfg(test)]
    fn new(context: &ConversationArchiveContext) -> Self {
        Self {
            agent_id: context.agent_id.clone(),
            provider: context.provider.clone(),
            provider_source_key: context.provider_source_key.clone(),
            provider_session_ids: context.provider_session_ids.clone(),
        }
    }

    #[cfg(test)]
    fn matches(&self, context: &ConversationArchiveContext) -> bool {
        self.agent_id == context.agent_id
            && self.provider == context.provider
            && self.provider_source_key == context.provider_source_key
            && self.provider_session_ids == context.provider_session_ids
    }
}

#[cfg(test)]
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

#[cfg(test)]
#[derive(Serialize, Deserialize)]
struct Selection {
    scope: String,
    index: Stamp,
    before: u64,
    seen: Option<String>,
}

#[cfg(test)]
#[derive(Serialize, Deserialize)]
struct Proof {
    generated: bool,
    input_binding: Option<String>,
}

#[cfg(test)]
#[derive(Serialize, Deserialize)]
struct BodyJob {
    event_start: u64,
    event_bytes: u64,
    row: Row,
    offset: u64,
}

#[cfg(test)]
impl BodyJob {
    #[cfg(test)]
    /// A queued job owns its chunks even when no current display row refers to
    /// them. Register the schema-defined body root on every unfinished write.
    fn put(&self, store: &mut Store) -> io::Result<String> {
        let dependencies = self
            .row
            .body
            .as_ref()
            .and_then(|body| body.root.as_ref())
            .map(|root| Dependency::Node(root.clone()))
            .into_iter()
            .collect();
        store.put_linked(self, dependencies)
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn selection_path(context: &ConversationArchiveContext) -> io::Result<PathBuf> {
    Ok(locations(&context.agent_id)?
        .0
        .with_file_name("chat-bootstrap-selection.json"))
}

#[cfg(test)]
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
        wardian_core::archive_vacancy::validate_vacancies(&directory, &entry.conversation_id)?;
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
            archive_display: (context.provider == "codex").then(Default::default),
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
                turns: Default::default(),
                committed_output_bytes: 0,
                capture_stamp: "saved-archive".into(),
                progress: "indexing".into(),
                source_coverage: None,
                parent: None,
                changes: Vec::new(),
                recent_start: 0,
                row_count: 0,
                recent_page: None,
                bootstrap: Some(progress),
            }),
            true,
        ));
    }
    selection.index.validate(&index_path)?;
    let pending = selection.before > 0;
    let roots = selection
        .seen
        .iter()
        .cloned()
        .map(Dependency::Node)
        .collect::<Vec<_>>();
    // A new owner resumes through this pointer, so its seen tree must be durable
    // before either the selection object or its pointer can publish it.
    store.finish_checkpoint(&roots)?;
    let reference = store.put(&selection)?;
    write_json_atomic(&pointer, &Some(reference))?;
    Ok((None, pending))
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
/// Archive relations combine saved display rows and allow member-to-group
/// presentation aliases. Existing native relations take precedence; native
/// source correspondence and detail authority remain with their existing owners.
fn apply_archive_display_updates(
    store: &mut Store,
    head: &mut Head,
    updates: Vec<(String, Option<super::super::chat_logical_index::Relation>)>,
    changes: &mut HashMap<String, String>,
) -> io::Result<()> {
    let mut admitted = Vec::new();
    for (key, mut relation) in updates {
        let Some(reference) = store.get(&head.root, &key)? else {
            continue;
        };
        let row: Row = store.read(&reference)?;
        if row
            .relation
            .as_ref()
            .is_some_and(|current| !current.id.starts_with("archive-display:"))
        {
            continue;
        }
        if let Some(relation) = relation.as_mut() {
            relation.id = format!("archive-display:{}", relation.id);
        }
        admitted.push((key, relation));
    }
    apply_logical_updates(store, head, admitted, changes)
}

#[cfg(test)]
/// A warm projection has no bootstrap state. After restart, only a matching
/// saved archive without current native authority may replace that derived head.
pub(super) fn can_upgrade_saved_head(
    context: &ConversationArchiveContext,
    head: &Head,
) -> io::Result<bool> {
    if context.provider != "codex"
        || super::super::chat_logical_index::source_proof(context)?.is_some()
    {
        return Ok(false);
    }
    let directory =
        super::super::storage::conversation_dir(&context.agent_id, &head.conversation_id)?;
    let manifest: ConversationManifest = match bounded_json(&directory.join("manifest.json")) {
        Ok(manifest) => manifest,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if manifest.agent_id != context.agent_id
        || manifest.conversation_id != head.conversation_id
        || manifest.provider != context.provider
        || manifest.status != ConversationStatus::Open
        || manifest.effective_logging != ConversationLoggingSetting::Enabled
        || manifest.provider_source_key != context.provider_source_key
        || !context
            .provider_session_ids
            .iter()
            .all(|id| manifest.provider_session_ids.contains(id))
    {
        return Ok(false);
    }
    // A derived-only head without its saved input must stay readable. The
    // ordinary selector will validate fresh stamps again during bounded replay.
    for path in [
        super::super::storage::index_path(&context.agent_id)?,
        directory.join("events.jsonl"),
        directory.join("conversation.jsonl"),
    ] {
        match Stamp::read(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

#[cfg(test)]
fn archive_counts_complete(progress: &Progress) -> bool {
    progress.narrative_before == 0
        && progress.event_before == 0
        && progress.archive_display.as_ref().is_some_and(|index| {
            index.narratives_completed(progress.events.length) && !index.narrative_pending()
        })
}

#[cfg(test)]
pub(super) fn archive_display_ready(context: &ConversationArchiveContext) -> io::Result<bool> {
    let Some(reference) = head(&context.agent_id)? else {
        return Ok(false);
    };
    let mut store = Store::new(&locations(&context.agent_id)?.1);
    let published: Head = store.read(&reference)?;
    if published.agent_id != context.agent_id
        || published.source_key != context.provider_source_key
        || !scope_matches(&published, context)
        || published.progress != "ready"
    {
        return Ok(false);
    }
    let directory =
        super::super::storage::conversation_dir(&context.agent_id, &published.conversation_id)?;
    wardian_core::archive_vacancy::validate_vacancies(&directory, &published.conversation_id)?;
    Ok(published
        .bootstrap
        .as_ref()
        .is_some_and(archive_counts_complete))
}

#[cfg(test)]
/// A complete saved cohort grants archive paging only. A warm candidate or a
/// partial recount cannot bypass native cursor/renewal qualification.
pub(super) fn archive_only_head(head: &Head, context: &ConversationArchiveContext) -> bool {
    context.provider == "codex"
        && head.agent_id == context.agent_id
        && head.source_key == context.provider_source_key
        && scope_matches(head, context)
        && head.progress == "ready"
        && head.source_epoch.is_none()
        && !head.logical.has_evidence()
        && head.bootstrap.as_ref().is_some_and(|progress| {
            // Completion belongs to the exact physical cohort. A close, clear
            // or append may precede publication of the replacement Head.
            let current = || -> io::Result<()> {
                let directory = super::super::storage::conversation_dir(
                    &context.agent_id,
                    &head.conversation_id,
                )?;
                progress
                    .index
                    .validate(&super::super::storage::index_path(&context.agent_id)?)?;
                progress
                    .manifest
                    .validate(&directory.join("manifest.json"))?;
                progress
                    .narrative
                    .validate(&directory.join("conversation.jsonl"))?;
                progress.events.validate(&directory.join("events.jsonl"))
            };
            archive_counts_complete(progress) && current().is_ok()
        })
}

#[cfg(test)]
pub(super) fn advance(context: &ConversationArchiveContext) -> io::Result<bool> {
    let (head_path, objects) = locations(&context.agent_id)?;
    let mut store = Store::checkpoint_writer(&objects);
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
        if can_upgrade_saved_head(context, &published)? {
            retire(&context.agent_id)?;
            return Ok(true);
        }
        return Ok(false);
    };
    if context.provider == "codex" && progress.archive_display.is_none() {
        // Old settled saved checkpoints have never counted archive-only
        // relations. Reselect into a new derived generation and bounded scan.
        retire(&context.agent_id)?;
        return Ok(true);
    }
    let directory =
        super::super::storage::conversation_dir(&context.agent_id, &published.conversation_id)?;
    let index_path = super::super::storage::index_path(&context.agent_id)?;
    wardian_core::archive_vacancy::validate_vacancies(&directory, &published.conversation_id)?;
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
    if published.progress == "ready"
        && published.source_epoch.is_none()
        && !published.logical.has_evidence()
        && source_proof.is_none()
        && archive_counts_complete(&progress)
    {
        // The stamped archive cohort is unchanged and already qualified. A
        // quiet pass must not create a new revision or disturb pinned details.
        return Ok(false);
    }
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
            if let Some(index) = progress.archive_display.as_mut() {
                let updates = index.observe_archive(
                    &mut store,
                    &super::super::chat_logical_index::ArchiveAdmission {
                        context,
                        conversation: &published.conversation_id,
                        generation: &published.generation,
                        extent: progress.events.length,
                    },
                    &event,
                    &format!("{offset:020}"),
                )?;
                apply_archive_display_updates(&mut store, &mut published, updates, &mut changes)?;
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
                let job = BodyJob {
                    event_start: offset,
                    event_bytes: bytes.len() as u64,
                    row: row.clone(),
                    offset: 0,
                }
                .put(&mut store)?;
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
    // Spend at most one window of body bytes across bounded short jobs. Keep
    // the original global four-chunk reservation; extra chunks/jobs yield at
    // the soft limit so row, pending-job and Head publication retain headroom.
    let mut body_chunks = 0;
    let mut body_envelope_bytes = 0;
    for job_index in 0..BODY_JOBS_PER_PASS {
        if body_chunks == BODY_CHUNKS_PER_PASS || (job_index > 0 && store.checkpoint_due()) {
            break;
        }
        let Some((key, reference)) = store
            .page(&progress.bodies, progress.body_before.as_deref(), 1)?
            .into_iter()
            .next()
        else {
            break;
        };
        let mut job: BodyJob = store.read(&reference)?;
        let candidate = candidate(context, &published.conversation_id);
        if job.event_bytes > WINDOW_BYTES
            || job.event_start.saturating_add(job.event_bytes) > progress.events.length
        {
            return Err(io::Error::other("invalid saved chat body coordinates"));
        }
        // Validate before deferring: an invalid envelope must fail instead of
        // being left at the front of the queue forever. A valid large envelope
        // gets the full allowance when it becomes the next pass's first job.
        if job.event_bytes > WINDOW_BYTES - body_envelope_bytes {
            break;
        }
        body_envelope_bytes += job.event_bytes;
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
            while body_chunks < BODY_CHUNKS_PER_PASS {
                if body_chunks >= RESERVED_BODY_CHUNKS && store.checkpoint_due() {
                    break;
                }
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
                body_chunks += 1;
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
            let reference = job.put(&mut store)?;
            progress.bodies = Some(store.insert(&progress.bodies, &key, &reference)?);
            // Keep this incomplete job first. Neither budget exhaustion nor a
            // long body can allow a later job to move the persistent cursor.
            break;
        }
    }
    let body_pending = !store
        .page(&progress.bodies, progress.body_before.as_deref(), 1)?
        .is_empty();
    if progress.narrative_before == 0 && progress.event_before == 0 {
        if let Some(index) = progress.archive_display.as_mut() {
            index.qualify_narratives(progress.events.length);
            if !store.checkpoint_due() {
                let updates = index.activate_narratives(&mut store, 2)?;
                apply_archive_display_updates(&mut store, &mut published, updates, &mut changes)?;
            }
        }
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
        || progress
            .archive_display
            .as_ref()
            .is_some_and(|index| index.narrative_pending())
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
    published.recent_page = None;
    // Delta history is optional: dropping it makes old revisions request the
    // existing full-page reset. Required roots still obey the object limit.
    if serde_json::to_vec(&published)
        .map_err(io::Error::other)?
        .len()
        > super::super::chat_read_store::OBJECT_BYTES
    {
        published.parent = None;
        published.changes.clear();
    }
    published.recent_page = super::packed_recent::prepare(&mut store, &published)?;
    store.finish_checkpoint(&published.checkpoint_dependencies())?;
    let reference = store.put(&published)?;
    // LAST: cursors, provenance roots, rows and body chunks are durable together.
    write_json_atomic(&head_path, &reference)?;
    Ok(pending)
}

#[cfg(test)]
pub(super) fn retire(agent_id: &str) -> io::Result<()> {
    let path = locations(agent_id)?.0;
    write_json_atomic(&path, &Option::<String>::None)?;
    write_json_atomic(
        &path.with_file_name("chat-bootstrap-selection.json"),
        &Option::<String>::None,
    )
}

#[cfg(test)]
#[path = "chat_archive_display_tests.rs"]
mod archive_display_tests;

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
        let store = Store::checkpoint_writer(temp.path());
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

    #[test]
    fn cold_selection_partial_flush_preserves_pointer_and_restarts() {
        use crate::state::conversation_archive::tests::isolated_home;
        use wardian_core::conversations::ConversationBoundaryReason;

        let (_guard, _temp) = isolated_home();
        let context = ConversationArchiveContext::for_agent_id("cold-selection", "unknown");
        let entries = (0..26)
            .map(|index| ConversationIndexEntry {
                schema: 1,
                conversation_id: format!("closed-{index}"),
                agent_id: context.agent_id.clone(),
                agent_name: context.agent_name.clone(),
                agent_class: String::new(),
                workspace: String::new(),
                provider: context.provider.clone(),
                provider_session_ids: Vec::new(),
                started_at: String::new(),
                ended_at: None,
                status: ConversationStatus::Closed,
                boundary_reason: ConversationBoundaryReason::Shutdown,
                first_prompt_excerpt: None,
                last_record_excerpt: None,
                record_count: 0,
                turn_count: 0,
                has_turns: false,
                lifecycle_only: false,
                artifact_count: 0,
                path: String::new(),
            })
            .collect::<Vec<_>>();
        let index_path = super::super::super::storage::index_path(&context.agent_id).unwrap();
        std::fs::create_dir_all(index_path.parent().unwrap()).unwrap();
        let journal = entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap() + "\n")
            .collect::<String>();
        std::fs::write(&index_path, journal).unwrap();
        let objects = locations(&context.agent_id).unwrap().1;
        let mut first = Store::checkpoint_writer(&objects);
        let (selected, pending) = select(&context, &mut first).unwrap();
        assert!(selected.is_none() && pending);
        drop(first);
        let pointer = selection_path(&context).unwrap();
        let prior_pointer = std::fs::read(&pointer).unwrap();
        let prior_ref: Option<String> = bounded_json(&pointer).unwrap();
        let mut restart = Store::new(&objects);
        let prior: Selection = restart.read(prior_ref.as_ref().unwrap()).unwrap();
        assert_eq!(restart.page(&prior.seen, None, 80).unwrap().len(), 24);

        // Locate the final node for a real filesystem flush collision. The
        // previous selection stays published while some new children flush.
        let mut rehearsal = Store::checkpoint_writer(&objects);
        let mut final_root = prior.seen.clone();
        for entry in entries[..2].iter().rev() {
            let marker = rehearsal.put(&true).unwrap();
            final_root = Some(
                rehearsal
                    .insert(
                        &final_root,
                        &digest(entry.conversation_id.as_bytes()),
                        &marker,
                    )
                    .unwrap(),
            );
        }
        let blocked = objects.join(final_root.as_ref().unwrap());
        assert!(!blocked.exists());
        drop(rehearsal);
        std::fs::create_dir(&blocked).unwrap();
        let files = || {
            std::fs::read_dir(&objects)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_file())
                .count()
        };
        let before = files();
        let mut failing = Store::checkpoint_writer(&objects);
        assert!(select(&context, &mut failing).is_err());
        assert!(files() > before, "the failure must follow a partial flush");
        assert_eq!(std::fs::read(&pointer).unwrap(), prior_pointer);
        assert_eq!(
            Store::new(&objects)
                .page(&prior.seen, None, 80)
                .unwrap()
                .len(),
            24
        );
        drop(failing);
        std::fs::remove_dir(&blocked).unwrap();

        let mut retry = Store::checkpoint_writer(&objects);
        let (selected, pending) = select(&context, &mut retry).unwrap();
        assert!(selected.is_none() && !pending);
        drop(retry);
        let reference: Option<String> = bounded_json(&pointer).unwrap();
        let mut reader = Store::new(&objects);
        let saved: Selection = reader.read(reference.as_ref().unwrap()).unwrap();
        assert_eq!(saved.before, 0);
        assert_eq!(reader.page(&saved.seen, None, 80).unwrap().len(), 26);
        for entry in entries {
            assert!(reader
                .get(&saved.seen, &digest(entry.conversation_id.as_bytes()))
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn cold_body_jobs_keep_private_chunks_reachable_without_a_display_row() {
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        let mut first = Store::checkpoint_writer(&objects);
        let chunk = first.put_bytes(b"first chunk").unwrap();
        let body_root = first.insert(&None, "00000000000000000000", &chunk).unwrap();
        let mut job = BodyJob {
            event_start: 0,
            event_bytes: 1,
            row: Row {
                agent_id: "cold-agent".into(),
                conversation_id: "saved-conversation".into(),
                key: "00000000000000000000".into(),
                event: serde_json::from_value(json!({
                    "id":"owned-body", "session_id":"cold-agent",
                    "provider":"unknown", "kind":"message"
                }))
                .unwrap(),
                body: Some(Body {
                    root: Some(body_root.clone()),
                    ..Default::default()
                }),
                relation: None,
                removed_display_ids: Vec::new(),
            },
            offset: 11,
        };
        let reference = job.put(&mut first).unwrap();
        let jobs = first.insert(&None, &job.row.key, &reference).unwrap();
        assert!(!objects.join(&body_root).exists());
        first
            .finish_checkpoint(&[Dependency::Node(jobs.clone())])
            .unwrap();
        drop(first);
        let mut restart = Store::checkpoint_writer(&objects);
        let reference = restart.get(&Some(jobs), &job.row.key).unwrap().unwrap();
        job = restart.read(&reference).unwrap();
        assert_eq!(
            job.row.body.as_ref().unwrap().root.as_ref(),
            Some(&body_root)
        );
        assert_eq!(
            restart
                .get(&Some(body_root.clone()), "00000000000000000000")
                .unwrap(),
            Some(chunk)
        );
        let next_chunk = restart.put_bytes(b"next chunk").unwrap();
        let next_root = restart
            .insert(&Some(body_root), "00000000000000000011", &next_chunk)
            .unwrap();
        job.row.body.as_mut().unwrap().root = Some(next_root.clone());
        job.offset = 21;
        let next_job = job.put(&mut restart).unwrap();
        let jobs = restart.insert(&None, &job.row.key, &next_job).unwrap();
        assert!(!objects.join(&next_root).exists());
        restart
            .finish_checkpoint(&[Dependency::Node(jobs.clone())])
            .unwrap();
        drop(restart);
        let mut reader = Store::new(&objects);
        let reference = reader.get(&Some(jobs), &job.row.key).unwrap().unwrap();
        let saved: BodyJob = reader.read(&reference).unwrap();
        assert_eq!(saved.offset, 21);
        assert_eq!(
            saved.row.body.as_ref().unwrap().root.as_ref(),
            Some(&next_root)
        );
        assert_eq!(reader.page(&Some(next_root), None, 2).unwrap().len(), 2);
    }

    fn cold_body_drain_fixture(
        body_padding: &[usize],
        envelope_padding: &[usize],
    ) -> (
        ConversationArchiveContext,
        Vec<Row>,
        HashMap<String, String>,
    ) {
        assert_eq!(body_padding.len(), envelope_padding.len());
        let context = ConversationArchiveContext::for_agent_id("cold-body-drain", "codex");
        let conversation_id = "saved-body-drain";
        let directory =
            super::super::super::storage::conversation_dir(&context.agent_id, conversation_id)
                .unwrap();
        fs::create_dir_all(directory.join("artifacts")).unwrap();
        let index_path = super::super::super::storage::index_path(&context.agent_id).unwrap();
        fs::write(&index_path, b"").unwrap();
        fs::write(directory.join("manifest.json"), b"{}").unwrap();
        fs::write(directory.join("conversation.jsonl"), b"").unwrap();

        let (head_path, objects) = locations(&context.agent_id).unwrap();
        let mut store = Store::checkpoint_writer(&objects);
        let candidate = candidate(&context, conversation_id);
        let mut envelopes = Vec::new();
        let mut rows = Vec::new();
        let mut jobs = None;
        let mut expected = HashMap::new();
        for (index, &body_bytes) in body_padding.iter().enumerate() {
            let id = format!("cold-body-{index}");
            let artifact = format!("body-{index}.txt");
            let text = format!("Body {index}: {}", "x".repeat(body_bytes));
            fs::write(directory.join("artifacts").join(&artifact), text.as_bytes()).unwrap();
            let event: AgentChatEvent = serde_json::from_value(json!({
                "id": id,
                "session_id": context.agent_id,
                "provider": "codex",
                "kind": "message",
                "role": "user",
                "text": format!("Body {index}"),
                "metadata": {"text_artifact_refs": [artifact], "text_excerpt": true}
            }))
            .unwrap();
            let event_start = envelopes.len() as u64;
            let mut envelope = serde_json::to_vec(&event).unwrap();
            envelope.resize(envelope.len() + envelope_padding[index], b' ');
            envelope.push(b'\n');
            assert!(envelope.len() <= WINDOW_BYTES as usize);
            let source = body_source(&candidate, &event).unwrap().unwrap();
            let row = Row {
                agent_id: context.agent_id.clone(),
                conversation_id: conversation_id.into(),
                key: format!("{event_start:020}"),
                event: header(&event),
                body: Some(Body {
                    binding: source.binding().unwrap(),
                    ..Default::default()
                }),
                relation: None,
                removed_display_ids: Vec::new(),
            };
            let job = BodyJob {
                event_start,
                event_bytes: envelope.len() as u64,
                row: row.clone(),
                offset: 0,
            }
            .put(&mut store)
            .unwrap();
            jobs = Some(store.insert(&jobs, &row.key, &job).unwrap());
            envelopes.extend(envelope);
            expected.insert(event.id.clone(), text);
            rows.push(row);
        }
        let event_path = directory.join("events.jsonl");
        fs::write(&event_path, &envelopes).unwrap();

        // Seed an admitted cohort with metadata exhausted, isolating body drain
        // from selection, proof scanning, header admission and relation work.
        let mut archive_display = super::super::super::chat_logical_index::Index::default();
        archive_display.qualify_narratives(envelopes.len() as u64);
        let mut saved = Head {
            agent_id: context.agent_id.clone(),
            conversation_id: conversation_id.into(),
            source_key: None,
            generation: "body-drain-generation".into(),
            source_epoch: None,
            root: None,
            identities: None,
            logical: Default::default(),
            turns: Default::default(),
            committed_output_bytes: envelopes.len() as u64,
            capture_stamp: "saved-archive".into(),
            progress: "indexing".into(),
            source_coverage: None,
            parent: None,
            changes: Vec::new(),
            recent_start: 0,
            row_count: envelopes.len(),
            recent_page: None,
            bootstrap: Some(Progress {
                selection_scope: Some(SelectionScope::new(&context)),
                index: Stamp::read(&index_path).unwrap(),
                manifest: Stamp::read(&directory.join("manifest.json")).unwrap(),
                narrative: Stamp::read(&directory.join("conversation.jsonl")).unwrap(),
                events: Stamp::read(&event_path).unwrap(),
                narrative_before: 0,
                event_before: 0,
                proofs: None,
                bodies: jobs,
                body_before: None,
                archive_display: Some(archive_display),
            }),
        };
        for row in &rows {
            insert_row(&mut store, &mut saved, row).unwrap();
        }
        store
            .finish_checkpoint(&saved.checkpoint_dependencies())
            .unwrap();
        let reference = store.put(&saved).unwrap();
        write_json_atomic(&head_path, &reference).unwrap();
        drop(store);
        (context, rows, expected)
    }

    fn cold_body_drain_rows(context: &ConversationArchiveContext) -> (Head, Vec<Row>) {
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        let published: Head = reader
            .read(&head(&context.agent_id).unwrap().unwrap())
            .unwrap();
        let rows = reader
            .page(&published.root, None, PAGE_ROWS)
            .unwrap()
            .into_iter()
            .map(|(_, reference)| reader.read(&reference).unwrap())
            .collect();
        (published, rows)
    }

    fn assert_cold_body_drain_payloads(
        context: &ConversationArchiveContext,
        expected: &HashMap<String, String>,
    ) {
        let (_, rows) = cold_body_drain_rows(context);
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        for (id, text) in expected {
            let row = rows.iter().find(|row| &row.event.id == id).unwrap();
            let body = row.body.as_ref().unwrap();
            assert!(body.complete);
            assert_eq!(body.bytes, text.len() as u64);
            let chunks = reader.page(&body.root, None, PAGE_ROWS).unwrap();
            let mut actual = Vec::new();
            for (_, reference) in chunks.into_iter().rev() {
                actual.extend(reader.read_bytes(&reference, BODY_BYTES).unwrap());
            }
            assert_eq!(actual.as_slice(), text.as_bytes());
        }
    }

    #[test]
    fn cold_body_drain_envelope_budget_waits_without_skipping_a_large_valid_job() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) =
            cold_body_drain_fixture(&[8 * 1024; 3], &[0, WINDOW_BYTES as usize - 2048, 4096]);
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        let initial: Head = reader
            .read(&head(&context.agent_id).unwrap().unwrap())
            .unwrap();
        let jobs = reader
            .page(&initial.bootstrap.as_ref().unwrap().bodies, None, 3)
            .unwrap()
            .into_iter()
            .map(|(_, reference)| reader.read::<BodyJob>(&reference).unwrap())
            .collect::<Vec<_>>();
        assert!(jobs[1].event_bytes <= WINDOW_BYTES);
        assert!(jobs[0].event_bytes + jobs[1].event_bytes > WINDOW_BYTES);
        assert!(jobs[0].event_bytes + jobs[2].event_bytes < WINDOW_BYTES);
        drop(reader);

        assert!(advance(&context).unwrap());
        let (partial, published) = cold_body_drain_rows(&context);
        assert_eq!(
            partial.bootstrap.as_ref().unwrap().body_before.as_ref(),
            Some(&rows[2].key)
        );
        for row in published {
            assert_eq!(
                row.body.as_ref().unwrap().complete,
                row.event.id == rows[2].event.id
            );
        }
        // The deferred envelope is now first and receives the full allowance.
        assert!(!advance(&context).unwrap());
        assert_cold_body_drain_payloads(&context, &expected);
    }

    #[test]
    fn cold_body_drain_invalid_coordinates_error_before_envelope_budget_deferral() {
        use crate::state::conversation_archive::tests::isolated_home;

        for oversized in [false, true] {
            let (_guard, temp) = isolated_home();
            let (context, rows, expected) =
                cold_body_drain_fixture(&[8 * 1024; 3], &[0, WINDOW_BYTES as usize - 2048, 4096]);
            let (head_path, objects) = locations(&context.agent_id).unwrap();
            assert!(head_path.starts_with(temp.path()));
            let mut writer = Store::checkpoint_writer(&objects);
            let mut saved: Head = writer
                .read(&head(&context.agent_id).unwrap().unwrap())
                .unwrap();
            let progress = saved.bootstrap.as_mut().unwrap();
            let key = &rows[1].key;
            let reference = writer.get(&progress.bodies, key).unwrap().unwrap();
            let mut job: BodyJob = writer.read(&reference).unwrap();
            if oversized {
                job.event_bytes = WINDOW_BYTES + 1;
            } else {
                job.event_start = progress.events.length;
            }
            let reference = job.put(&mut writer).unwrap();
            progress.bodies = Some(writer.insert(&progress.bodies, key, &reference).unwrap());
            writer
                .finish_checkpoint(&saved.checkpoint_dependencies())
                .unwrap();
            let reference = writer.put(&saved).unwrap();
            write_json_atomic(&head_path, &reference).unwrap();
            drop(writer);
            let before = fs::read(&head_path).unwrap();

            assert!(advance(&context)
                .unwrap_err()
                .to_string()
                .contains("invalid saved chat body coordinates"));
            assert_eq!(fs::read(&head_path).unwrap(), before);
            assert!(objects
                .join(digest(expected[&rows[2].event.id].as_bytes()))
                .is_file());
        }
    }

    #[test]
    fn cold_body_drain_shares_chunk_budget_and_restarts_the_first_partial_job() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(
            &[8 * 1024, (BODY_CHUNKS_PER_PASS + 1) * BODY_BYTES, 8 * 1024],
            &[0; 3],
        );
        assert!(advance(&context).unwrap());
        let (partial, published) = cold_body_drain_rows(&context);
        let middle = published
            .iter()
            .find(|row| row.event.id == rows[1].event.id)
            .unwrap();
        assert!(!middle.body.as_ref().unwrap().complete);
        assert_eq!(
            middle.body.as_ref().unwrap().bytes,
            (BODY_CHUNKS_PER_PASS - 1) as u64 * BODY_BYTES as u64
        );
        assert!(
            published
                .iter()
                .map(|row| row.body.as_ref().unwrap().bytes)
                .sum::<u64>()
                <= BODY_CHUNKS_PER_PASS as u64 * BODY_BYTES as u64
        );
        assert!(
            !published
                .iter()
                .find(|row| row.event.id == rows[0].event.id)
                .unwrap()
                .body
                .as_ref()
                .unwrap()
                .complete
        );
        let progress = partial.bootstrap.as_ref().unwrap();
        assert_eq!(progress.body_before.as_ref(), Some(&rows[2].key));
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        let reference = reader.get(&progress.bodies, &rows[1].key).unwrap().unwrap();
        let saved: BodyJob = reader.read(&reference).unwrap();
        assert_eq!(
            saved.offset,
            (BODY_CHUNKS_PER_PASS - 1) as u64 * BODY_BYTES as u64
        );
        drop(reader);

        assert!(!advance(&context).unwrap());
        assert_cold_body_drain_payloads(&context, &expected);
    }

    #[test]
    fn cold_body_drain_changed_second_source_preserves_the_previous_checkpoint() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(&[8 * 1024; 3], &[0; 3]);
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &rows[1].conversation_id,
        )
        .unwrap();
        // A changed length makes the binding mismatch deterministic even on
        // filesystems whose modification timestamps have coarse resolution.
        fs::write(directory.join("artifacts/body-1.txt"), b"changed source").unwrap();
        let (head_path, objects) = locations(&context.agent_id).unwrap();
        let before = fs::read(&head_path).unwrap();
        assert!(advance(&context)
            .unwrap_err()
            .to_string()
            .contains("saved chat body changed"));
        assert_eq!(fs::read(&head_path).unwrap(), before);
        assert!(objects
            .join(digest(expected[&rows[2].event.id].as_bytes()))
            .is_file());
    }

    #[test]
    fn cold_body_drain_missing_artifact_keeps_binding_and_obeys_job_limit() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, mut expected) = cold_body_drain_fixture(
            &[8 * 1024; BODY_JOBS_PER_PASS + 1],
            &[0; BODY_JOBS_PER_PASS + 1],
        );
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &rows[BODY_JOBS_PER_PASS].conversation_id,
        )
        .unwrap();
        fs::remove_file(directory.join(format!("artifacts/body-{BODY_JOBS_PER_PASS}.txt")))
            .unwrap();
        assert!(advance(&context).unwrap());
        let (partial, published) = cold_body_drain_rows(&context);
        assert_eq!(
            partial.bootstrap.as_ref().unwrap().body_before.as_ref(),
            Some(&rows[1].key)
        );
        let missing = published
            .iter()
            .find(|row| row.event.id == rows[BODY_JOBS_PER_PASS].event.id)
            .unwrap();
        assert_eq!(missing.event.metadata["chat_body_unavailable"], true);
        assert_eq!(
            missing.body.as_ref().unwrap().binding,
            rows[BODY_JOBS_PER_PASS].body.as_ref().unwrap().binding
        );
        assert!(!missing.body.as_ref().unwrap().complete);
        assert!(
            !published
                .iter()
                .find(|row| row.event.id == rows[0].event.id)
                .unwrap()
                .body
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(!advance(&context).unwrap());
        expected.remove(&rows[BODY_JOBS_PER_PASS].event.id);
        assert_cold_body_drain_payloads(&context, &expected);
    }

    #[test]
    fn cold_body_drain_packs_bounded_short_jobs_per_checkpoint_across_retry_and_restart() {
        use crate::state::conversation_archive::tests::isolated_home;
        use std::collections::HashSet;

        const JOBS: usize = 33;
        const CHUNKS_PER_CHECKPOINT: usize = 16;

        let (_guard, temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(&[8 * 1024; JOBS], &[0; JOBS]);
        let (head_path, objects) = locations(&context.agent_id).unwrap();
        assert!(expected.values().all(|text| text.len() <= BODY_BYTES));
        let generation = cold_body_drain_rows(&context).0.generation;

        // Fail the second job after an earlier chunk has been persisted. The
        // whole unpublished batch must retry from the previous head/cursor.
        let second = &rows[JOBS - 2];
        let blocked = objects.join(digest(expected[&second.event.id].as_bytes()));
        assert!(blocked.starts_with(temp.path()));
        assert!(!blocked.exists());
        fs::create_dir(&blocked).unwrap();
        let initial_pointer = fs::read(&head_path).unwrap();
        assert!(advance(&context).is_err());
        let first = rows.last().unwrap();
        assert!(objects
            .join(digest(expected[&first.event.id].as_bytes()))
            .is_file());
        assert_eq!(fs::read(&head_path).unwrap(), initial_pointer);
        fs::remove_dir(&blocked).unwrap();

        let mut completed = HashSet::new();
        let mut cursor = None;
        let mut productive_checkpoints = 0;
        let mut settled = false;
        for _ in 0..=JOBS {
            let pending = advance(&context).unwrap();
            // Reopen all persisted state after every checkpoint. No owner,
            // queued file handle or transient Store survives a restart.
            let mut reader = Store::new(&objects);
            let reference = head(&context.agent_id).unwrap().unwrap();
            let current: Head = reader.read(&reference).unwrap();
            assert_eq!(current.generation, generation);
            let progress = current.bootstrap.as_ref().unwrap();
            assert_eq!(progress.narrative_before, 0);
            assert_eq!(progress.event_before, 0);
            let mut next_completed = HashSet::new();
            let mut completed_bytes = 0;
            let published_rows = reader.page(&current.root, None, JOBS + 1).unwrap();
            assert_eq!(published_rows.len(), JOBS);
            for (_, reference) in published_rows {
                let row: Row = reader.read(&reference).unwrap();
                let body = row.body.as_ref().unwrap();
                if !body.complete {
                    continue;
                }
                let chunks = reader.page(&body.root, None, 2).unwrap();
                assert_eq!(chunks.len(), 1, "a one-chunk body must not be duplicated");
                assert_eq!(chunks[0].0, format!("{:020}", 0));
                let actual = reader.read_bytes(&chunks[0].1, BODY_BYTES).unwrap();
                assert_eq!(actual.as_slice(), expected[&row.event.id].as_bytes());
                assert_eq!(body.bytes, actual.len() as u64);
                if !completed.contains(&row.event.id) {
                    completed_bytes += actual.len();
                }
                next_completed.insert(row.event.id);
            }
            assert!(next_completed.is_superset(&completed));
            let finished = next_completed.len() - completed.len();
            assert!(finished <= CHUNKS_PER_CHECKPOINT);
            assert!(completed_bytes <= CHUNKS_PER_CHECKPOINT * BODY_BYTES);
            if finished > 0 {
                productive_checkpoints += 1;
                let next_cursor = progress.body_before.as_ref().unwrap();
                if let Some(previous) = &cursor {
                    assert!(
                        next_cursor < previous,
                        "body cursor must move only backward"
                    );
                }
            } else {
                assert_eq!(progress.body_before, cursor);
            }
            cursor = progress.body_before.clone();
            completed = next_completed;
            if !pending {
                assert_eq!(current.progress, "ready");
                assert!(reader
                    .page(&progress.bodies, progress.body_before.as_deref(), 1)
                    .unwrap()
                    .is_empty());
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "the finite body queue must settle without restarting"
        );
        assert_eq!(
            completed.len(),
            JOBS,
            "every artifact body must recover in full"
        );
        let settled_pointer = fs::read(&head_path).unwrap();
        assert!(!advance(&context).unwrap());
        assert_eq!(fs::read(&head_path).unwrap(), settled_pointer);
        let maximum = JOBS.div_ceil(CHUNKS_PER_CHECKPOINT);
        assert!(
            productive_checkpoints <= maximum,
            "{JOBS} one-chunk bodies took {productive_checkpoints} productive checkpoints \
             (maximum {maximum}); \
             the bounded body allowance must be shared across body jobs"
        );
    }

    fn cold_body_boundary_records(rows: &[Row]) -> Vec<ConversationNarrativeRecord> {
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let mut record =
                    super::super::super::narrative_from_chat_event(&row.event, index as u64 + 1)
                        .unwrap();
                record.at = "2026-01-01T00:00:00Z".into();
                record.artifact_refs = vec![format!("body-{index}.txt")];
                record
            })
            .collect()
    }

    // Replace the minimal body-only fixture's placeholders with a physically
    // bound saved cohort before exercising normal reads or metadata admission.
    fn cold_body_boundary_cohort(
        context: &ConversationArchiveContext,
        saved: &mut Head,
        records: &[ConversationNarrativeRecord],
    ) {
        use wardian_core::conversations::{ConversationBoundaryReason, ConversationFormatVersions};

        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &saved.conversation_id,
        )
        .unwrap();
        let manifest = ConversationManifest {
            schema: 1,
            conversation_id: saved.conversation_id.clone(),
            agent_id: context.agent_id.clone(),
            agent_name: context.agent_name.clone(),
            agent_class: context.agent_class.clone(),
            workspace: context.workspace.clone(),
            provider: context.provider.clone(),
            provider_session_ids: context.provider_session_ids.clone(),
            provider_source_key: context.provider_source_key.clone(),
            provider_native_refs: Vec::new(),
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            closed_at: None,
            status: ConversationStatus::Open,
            boundary_reason: ConversationBoundaryReason::Spawn,
            format_versions: ConversationFormatVersions::default(),
            record_count: records.len() as u64,
            turn_count: 0,
            has_turns: false,
            lifecycle_only: false,
        };
        let entry = super::super::super::storage::index_entry_from_manifest(
            &manifest,
            None,
            None,
            None,
            records.len() as u64,
            records
                .iter()
                .map(|record| record.artifact_refs.len() as u64)
                .sum(),
        );
        let index_path = super::super::super::storage::index_path(&context.agent_id).unwrap();
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(&index_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();
        fs::write(
            directory.join("conversation.jsonl"),
            records
                .iter()
                .map(|record| serde_json::to_string(record).unwrap() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let progress = saved.bootstrap.as_mut().unwrap();
        progress.selection_scope = Some(SelectionScope::new(context));
        progress.index = Stamp::read(&index_path).unwrap();
        progress.manifest = Stamp::read(&directory.join("manifest.json")).unwrap();
        progress.narrative = Stamp::read(&directory.join("conversation.jsonl")).unwrap();
        progress.events = Stamp::read(&directory.join("events.jsonl")).unwrap();
    }

    fn cold_body_boundary_publish(context: &ConversationArchiveContext, saved: &Head) -> String {
        let (pointer, objects) = locations(&context.agent_id).unwrap();
        let mut store = Store::checkpoint_writer(&objects);
        store
            .finish_checkpoint(&saved.checkpoint_dependencies())
            .unwrap();
        let reference = store.put(saved).unwrap();
        write_json_atomic(&pointer, &reference).unwrap();
        reference
    }

    #[test]
    fn cold_body_near_limit_head_resets_old_revision_and_drains_after_restart() {
        use super::super::super::chat_read_store::OBJECT_BYTES;
        use crate::state::conversation_archive::tests::isolated_home;

        const JOBS: usize = 17;
        let (_guard, temp) = isolated_home();
        let (mut context, rows, expected) = cold_body_drain_fixture(&[8 * 1024; JOBS], &[0; JOBS]);
        let (mut saved, _) = cold_body_drain_rows(&context);
        let records = cold_body_boundary_records(&rows);
        context.provider_session_ids = vec!["saved-session-000".into()];
        cold_body_boundary_cohort(&context, &mut saved, &records);

        // Use realistic bounded IDs, not an oversized arbitrary Head field.
        // Scope, caller context, manifest and index all retain the same list.
        let target = OBJECT_BYTES - 500;
        loop {
            saved.bootstrap.as_mut().unwrap().selection_scope = Some(SelectionScope::new(&context));
            let length = serde_json::to_vec(&saved).unwrap().len();
            assert!(length <= target);
            if target - length <= 128 {
                context
                    .provider_session_ids
                    .last_mut()
                    .unwrap()
                    .push_str(&"s".repeat(target - length));
                break;
            }
            context.provider_session_ids.push(format!(
                "saved-session-{:03}-{}",
                context.provider_session_ids.len(),
                "s".repeat(96)
            ));
        }
        assert!(context
            .provider_session_ids
            .iter()
            .all(|id| id.len() <= 256));
        cold_body_boundary_cohort(&context, &mut saved, &records);
        let initial_size = serde_json::to_vec(&saved).unwrap().len();
        assert!((OBJECT_BYTES - 520..=OBJECT_BYTES - 480).contains(&initial_size));
        assert!(scope_matches(&saved, &context));

        let mut four = saved.clone();
        four.parent = Some("0".repeat(64));
        four.changes = vec!["0".repeat(64); 4];
        four.bootstrap.as_mut().unwrap().body_before = Some(rows[JOBS - 4].key.clone());
        four.recent_page = Some("0".repeat(64));
        assert!(serde_json::to_vec(&four).unwrap().len() <= OBJECT_BYTES);
        let mut sixteen = four;
        sixteen.changes = vec!["0".repeat(64); 16];
        sixteen.recent_page = None;
        assert!(serde_json::to_vec(&sixteen).unwrap().len() > OBJECT_BYTES);

        let previous = cold_body_boundary_publish(&context, &saved);
        let (pointer, objects) = locations(&context.agent_id).unwrap();
        assert!(pointer.starts_with(temp.path()) && objects.starts_with(temp.path()));
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &saved.conversation_id,
        )
        .unwrap();
        let protected = [
            super::super::super::storage::index_path(&context.agent_id).unwrap(),
            directory.join("manifest.json"),
            directory.join("conversation.jsonl"),
            directory.join("events.jsonl"),
        ]
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        });
        let mut snapshot = super::super::tests::snapshot(&context);
        snapshot.resume_session = context.provider_session_ids.first().cloned();
        let read = |revision| {
            super::super::read_with_revalidation_and_limit(
                &context, &snapshot, None, revision, None, None, None,
            )
            .unwrap()
        };
        let initial = read(None);
        assert_eq!(initial.events.len(), JOBS);
        assert!(initial.revision.starts_with(&previous));

        assert!(advance(&context).expect("a large optional delta must not block body publication"));
        let (checkpoint, published_rows) = cold_body_drain_rows(&context);
        assert!(checkpoint.parent.is_none() && checkpoint.changes.is_empty());
        assert_eq!(checkpoint.generation, saved.generation);
        assert!(scope_matches(&checkpoint, &context));
        assert!(checkpoint.source_epoch.is_none());
        assert_eq!(
            published_rows
                .iter()
                .filter(|row| row.body.as_ref().unwrap().complete)
                .count(),
            16
        );
        let current = head(&context.agent_id).unwrap().unwrap();
        assert!(super::super::changes_since(
            &mut Store::new(&objects),
            &current,
            &checkpoint,
            &previous,
        )
        .unwrap()
        .is_none());
        let refreshed = read(Some(initial.revision.as_str()));
        assert!(refreshed.reset && !refreshed.unchanged);
        assert_eq!(refreshed.events.len(), JOBS);
        assert_eq!(
            refreshed
                .events
                .iter()
                .map(|event| event.id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
        );
        assert!(refreshed.source_epoch.is_none() && refreshed.recent_admission.is_none());
        assert!(refreshed.revalidated_ids.is_none());

        // Reopen from the persisted checkpoint; no transient Store survives.
        assert!(!advance(&context).unwrap());
        assert_cold_body_drain_payloads(&context, &expected);
        let settled = read(None);
        assert_eq!(settled.events.len(), JOBS);
        let quiet = read(Some(settled.revision.as_str()));
        assert!(quiet.unchanged && quiet.events.is_empty() && !quiet.reset);
        let settled_pointer = fs::read(&pointer).unwrap();
        assert!(!advance(&context).unwrap());
        assert_eq!(fs::read(&pointer).unwrap(), settled_pointer);
        for (path, bytes) in protected {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn cold_body_metadata_pressure_reserves_four_chunks_then_restarts_exactly() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(&[20 * BODY_BYTES], &[0]);
        let (mut saved, _) = cold_body_drain_rows(&context);
        let template = cold_body_boundary_records(&rows).pop().unwrap();
        let records = (0..ADMIT_ROWS)
            .map(|index| {
                let mut record = template.clone();
                record.seq = index as u64 + 1;
                record.event_refs = (0..16)
                    .map(|reference| format!("metadata-proof-{index:02}-{reference:02}"))
                    .collect();
                record.artifact_refs.clear();
                if index == ADMIT_ROWS - 1 {
                    record.event_refs[0] = rows[0].event.id.clone();
                    record.artifact_refs = template.artifact_refs.clone();
                }
                record
            })
            .collect::<Vec<_>>();
        cold_body_boundary_cohort(&context, &mut saved, &records);
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &saved.conversation_id,
        )
        .unwrap();
        let narrative_path = directory.join("conversation.jsonl");
        let progress = saved.bootstrap.as_mut().unwrap();
        progress.narrative_before = progress.narrative.length;
        assert_eq!(progress.event_before, 0);
        assert!(progress.narrative.length <= WINDOW_BYTES);
        let metadata_window = window(&narrative_path, progress.narrative_before).unwrap();
        assert_eq!(metadata_window.len(), ADMIT_ROWS);
        cold_body_boundary_publish(&context, &saved);

        assert!(advance(&context).unwrap());
        let (first, first_rows) = cold_body_drain_rows(&context);
        let progress = first.bootstrap.as_ref().unwrap();
        // All 24 narratives fit in this window. Only checkpoint_due can stop
        // the successful proof loop before its final offset reaches zero.
        assert!(progress.narrative_before > 0);
        let processed = metadata_window
            .iter()
            .position(|(offset, _)| *offset == progress.narrative_before)
            .unwrap()
            + 1;
        assert!(processed < metadata_window.len());
        let (pointer, objects) = locations(&context.agent_id).unwrap();
        let mut reader = Store::new(&objects);
        let proofs = reader
            .page(&progress.proofs, None, ADMIT_ROWS * 16)
            .unwrap();
        assert_eq!(proofs.len(), processed * 16);
        let proof_keys = metadata_window
            .iter()
            .take(processed)
            .flat_map(|(_, bytes)| {
                serde_json::from_slice::<ConversationNarrativeRecord>(bytes)
                    .unwrap()
                    .event_refs
            })
            .map(|id| digest(id.as_bytes()))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            proofs
                .into_iter()
                .map(|(key, _)| key)
                .collect::<std::collections::BTreeSet<_>>(),
            proof_keys
        );
        let body = first_rows[0].body.as_ref().unwrap();
        assert_eq!(
            body.bytes,
            4 * BODY_BYTES as u64,
            "chunk five must yield to real metadata pressure"
        );
        assert!(!body.complete);
        assert!(progress.body_before.is_none());
        let queued_ref = reader.get(&progress.bodies, &rows[0].key).unwrap().unwrap();
        let queued: BodyJob = reader.read(&queued_ref).unwrap();
        assert_eq!(queued.offset, body.bytes);
        assert_eq!(queued.row.body.as_ref().unwrap().root, body.root);
        drop(reader);

        let mut previous_bytes = body.bytes;
        let mut previous_narrative = progress.narrative_before;
        let mut settled = false;
        for _ in 0..ADMIT_ROWS + 24 {
            // Every advance reconstructs its Store and queued source from disk.
            let pending = advance(&context).unwrap();
            let (checkpoint, current_rows) = cold_body_drain_rows(&context);
            let progress = checkpoint.bootstrap.as_ref().unwrap();
            let body = current_rows[0].body.as_ref().unwrap();
            assert_eq!(checkpoint.generation, saved.generation);
            assert!(scope_matches(&checkpoint, &context));
            assert!(progress.narrative_before <= previous_narrative);
            assert!(
                body.bytes >= previous_bytes
                    && body.bytes <= expected[&rows[0].event.id].len() as u64
            );
            if !body.complete {
                assert!(progress.body_before.is_none());
                let mut reader = Store::new(&objects);
                let reference = reader.get(&progress.bodies, &rows[0].key).unwrap().unwrap();
                let job: BodyJob = reader.read(&reference).unwrap();
                assert_eq!(job.offset, body.bytes);
            } else {
                assert_eq!(progress.body_before.as_ref(), Some(&rows[0].key));
            }
            previous_bytes = body.bytes;
            previous_narrative = progress.narrative_before;
            if !pending {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "real metadata pressure and the finite long body must drain"
        );
        assert_cold_body_drain_payloads(&context, &expected);
        let settled_pointer = fs::read(&pointer).unwrap();
        assert!(!advance(&context).unwrap());
        assert_eq!(fs::read(&pointer).unwrap(), settled_pointer);
    }
}
