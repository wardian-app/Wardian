//! Exact recent-owner continuity across the preview/canonical representation switch.
use super::*;
use wardian_core::models::chat::AgentChatAlias;
use wardian_core::models::chat_presentation::{
    ChatPresentation, ChatPresentationV1, ChatPreviewBridge,
};

const REVISION_TAG: &str = ":preview-bridge-v1:";

/// The legacy reader appends unindexed native rows after canonical rows. Restore
/// their physical order before certifying a range, so an earlier lifecycle row
/// cannot create a false gap in a later turn. An unknown position remains a
/// barrier; never assign it a default rank that could move it out of a turn.
pub(super) fn order_source_snapshot(events: &mut [AgentChatEvent], head: &Head) -> bool {
    // Archive-only rows have no physical epoch. Their authenticated Row keys
    // provide order; unindexed/native rows cannot use this exception.
    if head.source_epoch.is_none()
        && events.iter().all(|event| {
            event.session_id == head.agent_id
                && event.metadata["_wardian_canonical_row"] == true
                && event.metadata["chat_page_key"]
                    .as_str()
                    .is_some_and(|key| !key.is_empty())
        })
    {
        events.sort_by_cached_key(|event| {
            event.metadata["chat_page_key"]
                .as_str()
                .expect("authenticated archive Row key")
                .to_owned()
        });
        return true;
    }
    let Some(epoch) = head.source_epoch.as_deref() else {
        return false;
    };
    let ordered = events.iter().all(|event| {
        event.session_id == head.agent_id
            && event.metadata["chat_source_epoch"].as_str() == Some(epoch)
            && event.metadata["chat_source_ref"]
                .as_str()
                .is_some_and(|reference| !reference.is_empty())
            && event.metadata["chat_source_start"]
                .as_u64()
                .zip(event.metadata["chat_source_end"].as_u64())
                .is_some_and(|(start, end)| start < end)
    });
    if ordered {
        events.sort_by_cached_key(presentation_order);
    }
    ordered
}

/// The marker retains a reader's representation choice, never read authority.
pub(super) fn split_revision(revision: &str) -> Option<(String, String)> {
    let (head, rest) = revision.split_once('.')?;
    let (seed, source) = rest.rsplit_once('~')?;
    let (seed, admission) = seed.rsplit_once(REVISION_TAG)?;
    uuid::Uuid::parse_str(admission).ok()?;
    Some((format!("{head}.{seed}~{source}"), admission.to_owned()))
}

fn stamp_revision(revision: &str, admission: &str) -> Option<String> {
    let (head, rest) = revision.split_once('.')?;
    let (seed, source) = rest.rsplit_once('~')?;
    Some(format!("{head}.{seed}{REVISION_TAG}{admission}~{source}"))
}

/// Classify a source reader's generation without treating it as canonical
/// identity. The reference comes from this operation's backend-produced page,
/// never a client hint. Binding identity still comes only from the published
/// archive Head after turn/item qualification.
fn has_native_source_generation(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    store: &mut Store,
    page: &AgentChatPage,
    published: &Head,
) -> io::Result<bool> {
    let Some((_, reference)) = page.revision.rsplit_once('~') else {
        return Ok(false);
    };
    if !valid_ref(reference) || published.source_key.is_none() {
        return Ok(false);
    }
    let source = super::super::chat_source_index::qualification(snapshot, store, reference)?;
    Ok(source.source_key == published.source_key
        && Some(source.epoch.as_str()) == published.source_epoch.as_deref()
        && Some(source.generation.as_str()) == page.generation.as_deref())
}

pub(super) fn stamp_page(page: &mut AgentChatPage, admission: &str) -> io::Result<()> {
    if split_revision(&page.revision).is_some_and(|(_, current)| current == admission) {
        return Ok(());
    }
    let revision = stamp_revision(&page.revision, admission)
        .ok_or_else(|| io::Error::other("invalid bound revision"))?;
    if let Some(value) = &mut page.presentation {
        if let Some(mut decoded) = value.decode_v1().map_err(io::Error::other)? {
            if let Some(detail) = &mut decoded.detail {
                detail.revision = revision.clone();
            }
            value.payload = serde_json::to_value(decoded).map_err(io::Error::other)?;
        }
    }
    page.revision = revision;
    Ok(())
}

/// Bounded certification retained by the existing recent admission, never by a client.
#[derive(Clone, Serialize)]
pub(super) struct Binding {
    conversation_id: String,
    generation: String,
    source_epoch: Option<String>,
    source_revision: String,
    openings: BTreeSet<String>,
    owners: super::preview_projection::Owners,
    presentation: super::preview_projection::State,
}

impl Binding {
    /// Presentation and owner metadata mirror the same source certificates.
    /// Charge the larger logical set; serialized duplication is charged in bytes.
    pub(super) fn record_count(&self) -> usize {
        self.presentation.record_count().max(
            self.owners
                .values()
                .map(|owners| owners.record_count())
                .sum(),
        )
    }

    pub(super) fn charge(&self) -> usize {
        serde_json::to_vec(self).map_or(IPC_BYTES, |bytes| bytes.len())
    }

    fn matches(&self, head: &Head) -> bool {
        self.conversation_id == head.conversation_id
            && self.generation == head.generation
            && self.source_epoch == head.source_epoch
    }
}

pub(super) struct Proof {
    head_ref: String,
    binding: Binding,
    backing: BTreeMap<String, String>,
    restored: Vec<Row>,
    resolution_bytes: usize,
    resolution_records: usize,
}

impl Proof {
    pub(super) fn new(head: Head, head_ref: String, rows: Vec<(String, AgentChatEvent)>) -> Self {
        let binding = Binding {
            conversation_id: head.conversation_id,
            generation: head.generation,
            source_epoch: head.source_epoch,
            source_revision: String::new(),
            openings: BTreeSet::new(),
            owners: Default::default(),
            presentation: Default::default(),
        };
        Self {
            head_ref,
            binding,
            restored: Vec::new(),
            resolution_bytes: 0,
            resolution_records: 0,
            backing: rows
                .into_iter()
                .map(|(key, event)| {
                    let physical = event.metadata["chat_source_ref"]
                        .as_str()
                        .unwrap_or(&event.id)
                        .to_owned();
                    (key, physical)
                })
                .collect(),
        }
    }
}

/// Resolve literal selectors in the already authorized source snapshot, then
/// use scoped archive lookup for selectors outside that bounded window.
/// Failed selectors expose neither an error nor lookup accounting to readers.
fn resolve_hints(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    hints: &[String],
    headers: &[AgentChatEvent],
    watch: Option<&Proof>,
    source: &AgentChatPage,
) -> io::Result<Option<Proof>> {
    let Some(reference) = head(&snapshot.session_id)? else {
        return Ok(None);
    };
    let mut store = store(&snapshot.session_id)?;
    let published: Head = store.read(&reference)?;
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    if published.agent_id != snapshot.session_id
        || published.source_key != context.provider_source_key
        || !cold_bootstrap::scope_matches(&published, &context)
        || source.session_id != snapshot.session_id
        || source.conversation_id.as_deref() != Some(published.conversation_id.as_str())
        || source.source_epoch != published.source_epoch
        || !source.revision.starts_with(&format!("{reference}."))
    {
        return Ok(None);
    }
    let headers: BTreeMap<_, _> = headers
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect();
    let mut proof = Proof::new(published.clone(), reference.clone(), Vec::new());
    let mut valid = true;
    let mut seeks = SeekCache::default();
    let Some(backings) = source_backings(&source.events) else {
        return Ok(None);
    };
    for hint in hints {
        let physical = headers
            .get(hint.as_str())
            .and_then(|event| event.metadata["chat_recent_key"].as_str())
            .and_then(|key| {
                watch
                    .filter(|proof| proof.head_ref == reference)?
                    .backing
                    .get(key)
            })
            .map_or(hint.as_str(), String::as_str);
        if let Some(target) = backings.get(physical) {
            let Some(event) = target else {
                return Ok(None);
            };
            // This is positive evidence from the same scoped producer read,
            // including native records not captured into the archive yet.
            // No offset, client key or missing-lookup inference grants proof.
            let physical = if event.id == physical {
                event.metadata["chat_source_ref"]
                    .as_str()
                    .unwrap_or(physical)
            } else {
                physical
            };
            proof.backing.insert(hint.clone(), physical.to_owned());
            continue;
        }
        let found = store.get_reusing_nodes(
            &published.identities,
            &digest(physical.as_bytes()),
            &mut seeks,
        )?;
        let row = found
            .as_ref()
            .map(|reference| store.read::<Row>(reference))
            .transpose()?;
        let scoped = if let (Some(row), Some(found)) = (row.as_ref(), found.as_ref()) {
            row.agent_id == snapshot.session_id
                && row.conversation_id == published.conversation_id
                && row.event.provider == snapshot.provider
                && row.event.metadata["chat_source_epoch"].as_str()
                    == published.source_epoch.as_deref()
                && (row.event.id == physical
                    || row.event.metadata["chat_source_ref"].as_str() == Some(physical))
                && store
                    .get_reusing_nodes(&published.root, &row.key, &mut seeks)?
                    .as_ref()
                    == Some(found)
        } else {
            false
        };
        valid &= scoped;
        if scoped {
            let row = row.unwrap();
            proof.backing.insert(hint.clone(), physical.to_owned());
            proof.restored.push(row);
        }
    }
    proof.resolution_bytes = store.bytes + 2 * HEAD_BYTES as usize;
    proof.resolution_records = store.objects;
    Ok(
        (valid && head(&snapshot.session_id)?.as_deref() == Some(reference.as_str()))
            .then_some(proof),
    )
}

type SourceBackings<'a> = BTreeMap<&'a str, Option<&'a AgentChatEvent>>;

/// Index only this already-read window. Repeated markers within one row are
/// harmless; a marker shared by distinct rows stays ambiguous. Borrow keys
/// rather than copying metadata, and bound temporary entries by the page cap.
pub(super) fn source_backings(events: &[AgentChatEvent]) -> Option<SourceBackings<'_>> {
    if events.len() > PAGE_ROWS {
        return None;
    }
    let mut backings = SourceBackings::new();
    let mut charge = 0usize;
    for event in events {
        let members = event.metadata["chat_display_member_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str());
        for physical in std::iter::once(event.id.as_str())
            .chain(event.metadata["chat_source_ref"].as_str())
            .chain(members)
        {
            if let Some(target) = backings.get_mut(physical) {
                if target.is_some_and(|prior| !std::ptr::eq(prior, event)) {
                    *target = None;
                }
            } else {
                charge = charge
                    .checked_add(physical.len())?
                    .checked_add(std::mem::size_of::<(&str, Option<&AgentChatEvent>)>())?;
                if charge > IPC_BYTES {
                    return None;
                }
                backings.insert(physical, Some(event));
            }
        }
    }
    Some(backings)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    First,
    Legacy,
    Bound,
}

pub(super) struct Selection<'a> {
    pub(super) mode: Mode,
    pub(super) hints: Option<&'a [String]>,
}

fn normalize_aliases(page: &mut AgentChatPage) -> bool {
    let Some(targets) = source_backings(&page.events) else {
        return false;
    };
    if targets.values().any(Option::is_none) {
        return false;
    }
    let mut aliases = BTreeMap::new();
    for alias in &page.aliases {
        // The ordinary reader may retain aliases outside its final bounded
        // window. A bridge publishes only exact backing in this snapshot.
        let Some(Some(target)) = targets.get(alias.canonical_id.as_str()) else {
            continue;
        };
        let canonical = target.id.as_str();
        if aliases
            .insert(alias.observation_id.as_str(), canonical)
            .is_some_and(|prior| prior != canonical)
        {
            return false;
        }
    }
    // Selectors choose identity anchors, never membership. Publish the exact
    // backing aliases for every returned source row in the qualified snapshot,
    // including rows that were not among the bounded selector subset.
    for (observation, target) in targets {
        let canonical = target.unwrap().id.as_str();
        if aliases
            .insert(observation, canonical)
            .is_some_and(|prior| prior != canonical)
        {
            return false;
        }
    }
    page.aliases = aliases
        .into_iter()
        .map(|(observation, canonical)| AgentChatAlias {
            observation_id: observation.to_owned(),
            canonical_id: canonical.to_owned(),
        })
        .collect();
    true
}

/// Literal handoff targets occupy the existing row budget even outside the
/// requested recent suffix. Never publish a bridge with a missing target.
fn retain_recent_window(
    events: &mut Vec<AgentChatEvent>,
    limit: usize,
    required: &BTreeSet<String>,
) -> bool {
    if required.len() > PAGE_ROWS
        || required
            .iter()
            .any(|id| !events.iter().any(|event| event.id == *id))
    {
        return false;
    }
    let mut recent_limit = limit;
    let start = loop {
        let start = recent_window_start(events, recent_limit);
        let count = events.len() - start
            + events[..start]
                .iter()
                .filter(|event| required.contains(&event.id))
                .count();
        if count <= PAGE_ROWS {
            break start;
        }
        if recent_limit == 0 {
            return false;
        }
        recent_limit = recent_limit.saturating_sub(count - PAGE_ROWS);
    };
    let mut index = 0;
    events.retain(|event| {
        let keep = index >= start || required.contains(&event.id);
        index += 1;
        keep
    });
    true
}

/// Ordinary canonical pagination remains inside the admitted view. Its normal
/// reader remains the sole authority for the events, cursor and detail refs.
/// Passive latest reads may acknowledge only the validated admission lifetime
/// when their empty or provisional window cannot carry canonical membership.
pub(super) fn continue_canonical(
    cache: &Arc<Mutex<RecentSlots>>,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    runtime: &Arc<Mutex<String>>,
    requested_admission: &str,
    page: &mut AgentChatPage,
    ordinary_recent: bool,
) -> io::Result<()> {
    let value = page
        .presentation
        .as_ref()
        .and_then(|value| value.decode_v1().ok().flatten());
    let unqualified = page.recent_admission.is_some()
        || value.as_ref().is_some_and(|value| {
            value.preview_bridge.is_some()
                || value.items.iter().any(|item| {
                    item.provenance
                        != wardian_core::models::chat_presentation::ChatItemProvenance::Canonical
                })
        })
        || page
            .events
            .iter()
            .any(|event| event.metadata["chat_provisional"] == true);
    let candidate = {
        let current = cache
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if current.global_boundaries > 0 || current.boundaries.contains_key(&snapshot.session_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bound admission retired",
            ));
        }
        current
            .slots
            .iter()
            .find(|slot| {
                slot.agent_id == snapshot.session_id
                    && slot.admission == requested_admission
                    && slot.published
                    && slot.binding.is_some()
                    && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                    && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
            })
            .and_then(|slot| {
                slot.binding
                    .clone()
                    .map(|binding| (slot.admission.clone(), binding, slot.native_source.clone()))
            })
    };
    let Some((admission, binding, native_source)) = candidate else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound admission retired",
        ));
    };
    if native_source
        .as_ref()
        .is_some_and(|source| !source.valid().unwrap_or(false))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound source retired",
        ));
    }
    let Some(reference) = head(&snapshot.session_id)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound Head retired",
        ));
    };
    let mut store = store(&snapshot.session_id)?;
    let published: Head = store.read(&reference)?;
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    if !binding.matches(&published)
        || !cold_bootstrap::scope_matches(&published, &context)
        || published.source_key != context.provider_source_key
        || page.conversation_id.as_deref() != Some(published.conversation_id.as_str())
        || page.generation.as_deref() != Some(published.generation.as_str())
        || page.source_epoch != published.source_epoch
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound source scope retired",
        ));
    }
    if page.revision.split('.').next() != Some(reference.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "canonical revision changed",
        ));
    }
    let value = if unqualified {
        None
    } else {
        match value {
            Some(value) => Some(value),
            None => published
                .turns
                .continuation_page(&mut store, &page.events)?,
        }
    };
    let (scope, bytes) = recent_scope(snapshot)?;
    if head(&snapshot.session_id)?.as_deref() != Some(reference.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "canonical revision changed",
        ));
    }
    let current = cache
        .lock()
        .map_err(|_| io::Error::other("recent owner poisoned"))?;
    if current.global_boundaries > 0
        || current.boundaries.contains_key(&snapshot.session_id)
        || !current.slots.iter().any(|slot| {
            slot.agent_id == snapshot.session_id
                && slot.admission == admission
                && slot.published
                && slot.scope == scope
                && slot.native_source == native_source
                && slot
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.matches(&published))
                && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound admission retired",
        ));
    }
    page.bytes_read += bytes + store.bytes + 2 * HEAD_BYTES as usize;
    page.records_decoded += store.objects;
    let Some(mut value) = value else {
        if ordinary_recent {
            let continuity = ChatPresentationV1 {
                turns: Vec::new(),
                items: Vec::new(),
                state_event_refs: Vec::new(),
                legacy_turns: Vec::new(),
                detail: None,
                preview_bridge: Some(ChatPreviewBridge {
                    admission_id: admission.clone(),
                    canonical_head: reference,
                }),
            };
            continuity.validate().map_err(io::Error::other)?;
            page.events.clear();
            page.aliases.clear();
            page.removed_ids.clear();
            page.next_before = None;
            page.detail = None;
            page.revalidated_ids = None;
            page.archive_read_ready = false;
            page.recent_admission = None;
            page.reset = false;
            page.older_page = false;
            page.presentation = Some(ChatPresentation {
                version: 1,
                payload: serde_json::to_value(continuity).map_err(io::Error::other)?,
            });
            stamp_page(page, &admission)?;
        }
        return Ok(());
    };
    for owners in binding.owners.values() {
        owners.preserve_item_orders(&mut value);
    }
    value.items.sort_by(|left, right| {
        (&left.order_key, &left.item_key).cmp(&(&right.order_key, &right.item_key))
    });
    value.preview_bridge = Some(ChatPreviewBridge {
        admission_id: admission.clone(),
        canonical_head: reference,
    });
    if !normalize_aliases(page) {
        return Ok(());
    }
    page.presentation = Some(ChatPresentation {
        version: 1,
        payload: serde_json::to_value(value).map_err(io::Error::other)?,
    });
    stamp_page(page, &admission)?;
    Ok(())
}

/// Select one authoritative source projection. The caller rechecks the receipt
/// after this IO and publishes the entire handoff under the recent-owner lock.
pub(super) fn read(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    admission: &str,
    headers: &[AgentChatEvent],
    prior: Option<&Binding>,
    proof: Option<&Proof>,
    limit: usize,
    selection: Selection<'_>,
) -> io::Result<Option<(AgentChatPage, Binding)>> {
    let Selection { mode, hints } = selection;
    if !(1..=PAGE_ROWS).contains(&limit) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid recent limit",
        ));
    }
    if !matches!(snapshot.provider.as_str(), "codex" | "claude") {
        return Ok(None);
    }
    let source_selectors = mode == Mode::Legacy && hints.is_some();
    // Before positive selector proof, IO failure cannot let an unrelated
    // Legacy reader retract another reader's shared certificate. The owner
    // treats this retry signal as an unchanged Legacy response.
    let unproven = |error| {
        if source_selectors {
            io::Error::new(io::ErrorKind::WouldBlock, "preview selectors unproven")
        } else {
            error
        }
    };
    let Some(reference) = head(&snapshot.session_id).map_err(&unproven)? else {
        return if prior.is_some() && !source_selectors {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bound Head retired",
            ))
        } else {
            Ok(None)
        };
    };
    let mut store = store(&snapshot.session_id).map_err(&unproven)?;
    let published: Head = store.read(&reference).map_err(&unproven)?;
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    if published.agent_id != snapshot.session_id
        || published.source_key != context.provider_source_key
        || !cold_bootstrap::scope_matches(&published, &context)
    {
        return if prior.is_some() && !source_selectors {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bound source scope retired",
            ))
        } else {
            Ok(None)
        };
    }
    if !source_selectors && prior.is_some_and(|binding| !binding.matches(&published)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound canonical scope changed",
        ));
    }
    // A binding is shared by readers. Only this request's legacy receipt can
    // select its handoff; another reader's switch never waives these aliases.
    let proof =
        proof.filter(|proof| proof.head_ref == reference && proof.binding.matches(&published));
    if (mode == Mode::Legacy && proof.is_none() && !source_selectors)
        || (mode == Mode::Bound && prior.is_none())
    {
        return Ok(None);
    }
    // Certification is shared by all readers of A. A reader's display limit
    // must never replace that source snapshot with a smaller certification.
    let mut page = read_preview_source(&context, snapshot, PAGE_ROWS).map_err(&unproven)?;
    if page.conversation_id.as_deref() != Some(published.conversation_id.as_str())
        || page.source_epoch != published.source_epoch
        || page.events.is_empty()
    {
        return Ok(None);
    }
    if !page.revision.starts_with(&format!("{reference}.")) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "canonical revision changed",
        ));
    }
    let native_generation = page.generation.as_deref() != Some(published.generation.as_str());
    if native_generation {
        page.bytes_read += POLICY_BYTES as usize;
        page.records_decoded += 1;
        if !has_native_source_generation(snapshot, &mut store, &page, &published)
            .map_err(&unproven)?
        {
            return Ok(None);
        }
    }
    // A shared certificate never waives this reader's literal selectors. A
    // failed selector returns Legacy without changing another reader's A.
    let selected = if source_selectors {
        resolve_hints(snapshot, hints.unwrap(), headers, proof, &page).unwrap_or(None)
    } else {
        None
    };
    let proof = if source_selectors {
        selected
            .as_ref()
            .filter(|proof| proof.head_ref == reference && proof.binding.matches(&published))
    } else {
        proof
    };
    if source_selectors && proof.is_none() {
        return Ok(None);
    }
    if prior.is_some_and(|binding| !binding.matches(&published)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound canonical scope changed",
        ));
    }
    let direct = prior.is_none() && proof.is_none() && mode == Mode::First;
    let Some(mut binding) = prior
        .cloned()
        .or_else(|| proof.map(|proof| proof.binding.clone()))
        .or_else(|| {
            direct.then(|| Binding {
                conversation_id: published.conversation_id.clone(),
                generation: published.generation.clone(),
                source_epoch: published.source_epoch.clone(),
                source_revision: String::new(),
                openings: BTreeSet::new(),
                owners: Default::default(),
                presentation: Default::default(),
            })
        })
    else {
        return Ok(None);
    };
    if !binding.matches(&published) {
        return Ok(None);
    }
    if let Some(proof) = proof {
        binding
            .openings
            .extend(proof.binding.openings.iter().cloned());
    }
    // Unqualified rows can change without changing turn certificates. Retain
    // the fixed source snapshot revision so the publication CAS fences those
    // older responses too, while identical polls remain compatible.
    binding.source_revision.clone_from(&page.revision);
    order_source_snapshot(&mut page.events, &published);
    if let Some(proof) = proof.filter(|_| mode == Mode::Legacy && hints.is_some()) {
        let mut required = BTreeSet::new();
        for row in &proof.restored {
            let reference = store
                .get(&published.root, &row.key)?
                .ok_or_else(|| io::Error::other("restored backing retired"))?;
            let mut row = row.clone();
            let original = row.event.metadata.clone();
            row.event = header(&row.event);
            for key in ["chat_turn_key", "chat_turn_item_key"] {
                if let Some(value) = original[key].as_str() {
                    row.event.metadata[key] = json!(value);
                }
            }
            let mut restored = display_event(row, &reference);
            if let Some(detail) = restored.metadata["chat_step_detail_ref"].as_str() {
                restored.metadata["chat_step_detail_ref"] =
                    json!(format!("{}:{detail}", proof.head_ref));
            }
            required.insert(restored.id.clone());
            if !page.events.iter().any(|event| event.id == restored.id) {
                page.events.push(restored);
            }
        }
        if !order_source_snapshot(&mut page.events, &published) {
            return Ok(None);
        }
        // Restored headers share the ordinary window's existing hard row cap.
        if !retain_recent_window(&mut page.events, PAGE_ROWS, &required) {
            return Ok(None);
        }
    }
    let canonical = page
        .presentation
        .as_ref()
        .and_then(|value| value.decode_v1().ok().flatten());
    let running = matches!(
        snapshot.current_status.to_ascii_lowercase().as_str(),
        "processing" | "running" | "working"
    );
    let Some((mut value, presentation, owners)) = super::preview_projection::build(
        super::preview_projection::Build {
            store: &mut store,
            head: &published,
            head_ref: &reference,
            admission,
            running,
            openings: &binding.openings,
            previous: &binding.presentation,
            owners: prior.map(|binding| &binding.owners),
            canonical: canonical.as_ref(),
            bound: mode == Mode::Bound,
        },
        &mut page.events,
    )?
    else {
        return Ok(None);
    };
    binding.presentation = presentation;
    binding.owners = owners;
    if native_generation
        || value.items.iter().any(|item| {
            item.provenance
                != wardian_core::models::chat_presentation::ChatItemProvenance::Canonical
        })
        || page.events.iter().any(|event| {
            event.metadata["chat_provisional"] == true
                || event.metadata["chat_identity_resolution"] == "unresolved"
        })
    {
        // Provisional content keeps the existing recent-admission authority floor.
        page.recent_admission = Some(admission.to_owned());
        page.progress = "provisional".into();
        page.next_before = None;
        page.older_page = false;
        page.detail = None;
        page.revalidated_ids = None;
        page.archive_read_ready = false;
        for event in &mut page.events {
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.remove("chat_detail_ref");
                metadata.remove("chat_body_binding");
            }
        }
    }
    let owned: BTreeSet<_> = value
        .items
        .iter()
        .flat_map(|item| item.event_refs.iter())
        .chain(value.state_event_refs.iter().map(|item| &item.event_id))
        .chain(value.legacy_turns.iter().flat_map(|turn| &turn.event_refs))
        .collect();
    if page.events.iter().any(|event| !owned.contains(&event.id)) {
        return Ok(None);
    }
    let Some(backings) = source_backings(&page.events) else {
        return Ok(None);
    };
    let mut aliases = Vec::new();
    let selectors: Vec<_> = hints.map_or_else(
        || {
            headers
                .iter()
                .map(|event| {
                    (
                        event.id.as_str(),
                        event.metadata["chat_recent_key"].as_str(),
                    )
                })
                .collect()
        },
        |hints| {
            hints
                .iter()
                .map(|id| (id.as_str(), Some(id.as_str())))
                .collect()
        },
    );
    for (observation, key) in selectors {
        if mode != Mode::Legacy {
            break;
        }
        let Some(key) = key else {
            return Ok(None);
        };
        let Some(physical) = proof.and_then(|proof| proof.backing.get(key)) else {
            return Ok(None);
        };
        let Some(Some(event)) = backings.get(physical.as_str()) else {
            return Ok(None);
        };
        aliases.push(AgentChatAlias {
            observation_id: observation.to_owned(),
            canonical_id: event.id.clone(),
        });
    }
    drop(backings);
    let required: BTreeSet<_> = aliases
        .iter()
        .map(|alias| alias.canonical_id.clone())
        .collect();
    if !retain_recent_window(&mut page.events, limit, &required) {
        return Ok(None);
    }
    let Some(window) = turn_read::window(value, &page.events, Some(&reference)) else {
        return Ok(None);
    };
    value = window;
    if page.recent_admission.is_none() {
        // A display limit or a restored selector may change the oldest Row.
        // Only its authenticated canonical key can grant an Older cursor.
        page.next_before = if let Some(key) = page
            .events
            .iter()
            .filter_map(|event| event.metadata["chat_page_key"].as_str())
            .min()
        {
            (!store.page(&published.root, Some(key), 1)?.is_empty()
                || published.progress == "indexing")
                .then(|| format!("{reference}:{key}"))
        } else {
            None
        };
    }
    value.preview_bridge = Some(ChatPreviewBridge {
        admission_id: admission.to_owned(),
        canonical_head: reference.clone(),
    });
    value.validate().map_err(io::Error::other)?;
    // A native transport generation never becomes canonical authority. Only
    // this fully qualified producer projection takes the published Head's G.
    page.generation = Some(published.generation.clone());
    page.presentation = Some(ChatPresentation {
        version: 1,
        payload: serde_json::to_value(&value).map_err(io::Error::other)?,
    });
    page.aliases.extend(aliases);
    if !normalize_aliases(&mut page) {
        return Ok(None);
    }
    page.reset = true;
    page.unchanged = false;
    stamp_page(&mut page, admission)?;
    // Retain only this bounded view's proven openings; do not grow a session index here.
    binding.openings = binding.presentation.openings();
    if binding.charge() > IPC_BYTES {
        binding.presentation.compact_gap_ids();
    }
    if binding.charge() > IPC_BYTES {
        return Ok(None);
    }
    if head(&snapshot.session_id)?.as_deref() != Some(reference.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "canonical revision changed",
        ));
    }
    page.bytes_read += store.bytes + 2 * HEAD_BYTES as usize;
    page.records_decoded += store.objects;
    if let Some(proof) = proof {
        page.bytes_read += proof.resolution_bytes;
        page.records_decoded += proof.resolution_records;
    }
    let page = finish(page)?;
    if page.presentation.is_none() {
        return Ok(None);
    }
    Ok(Some((page, binding)))
}
