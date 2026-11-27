// Legacy projection code retained for compatibility fixtures.
// This file is included only by the test build of its owner module.

const WINDOW_BYTES: u64 = 256 * 1024;

const ADMIT_ROWS: usize = 24;

const BODY_CHUNKS_PER_PASS: usize = 16;

const BODY_JOBS_PER_PASS: usize = 16;

const RESERVED_BODY_CHUNKS: usize = 4;

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

impl Progress {
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

impl BodyJob {
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

fn archive_counts_complete(progress: &Progress) -> bool {
    progress.narrative_before == 0
        && progress.event_before == 0
        && progress.archive_display.as_ref().is_some_and(|index| {
            index.narratives_completed(progress.events.length) && !index.narrative_pending()
        })
}

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
            for _ in 0..PAGE_ROWS {
                if !index.narrative_pending() || activation_full(&store, &changes) {
                    break;
                }
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
            for _ in 0..PAGE_ROWS {
                if !published.logical.narrative_pending() || activation_full(&store, &changes) {
                    break;
                }
                let updates = published.logical.activate_narratives(&mut store, 2)?;
                apply_logical_updates(&mut store, &mut published, updates, &mut changes)?;
            }
        }
        for _ in 0..PAGE_ROWS {
            if !published.logical.pending() || activation_full(&store, &changes) {
                break;
            }
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

pub(super) fn retire(agent_id: &str) -> io::Result<()> {
    let path = locations(agent_id)?.0;
    write_json_atomic(&path, &Option::<String>::None)?;
    write_json_atomic(
        &path.with_file_name("chat-bootstrap-selection.json"),
        &Option::<String>::None,
    )
}
