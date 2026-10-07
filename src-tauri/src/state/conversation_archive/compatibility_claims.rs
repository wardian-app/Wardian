//! Capture-only reservations for historically persisted Claude raw-line owners.
//! The private checkpoint publishes prepared roots before advancing its cursor.
use super::chat_read_store::{digest, valid_ref, Store, READ_OBJECTS};
use super::*;
use crate::commands::provider_log_acquisition::ProviderLogCaptureState;
use wardian_core::models::chat::AgentChatEventKind;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Checkpoint {
    version: u8,
    root: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Root {
    version: u8,
    owners: Option<String>,
    pending: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Claim {
    scope: String,
    canonical_id: String,
    coordinate: String,
    native_uuid: String,
    request_root_id: String,
    end: u64,
}

fn load(store: &mut Store, checkpoint: Option<&Checkpoint>) -> io::Result<Root> {
    let Some(checkpoint) = checkpoint else {
        return Ok(Root {
            version: 1,
            ..Root::default()
        });
    };
    if checkpoint.version != 1 || !valid_ref(&checkpoint.root) {
        return Err(io::Error::other("invalid compatibility checkpoint"));
    }
    let root: Root = store.read(&checkpoint.root)?;
    if root.version != 1 || root.owners.is_none() {
        return Err(io::Error::other("unsupported compatibility root"));
    }
    for reference in [&root.owners, &root.pending].into_iter().flatten() {
        if !valid_ref(reference) {
            return Err(io::Error::other("invalid compatibility seek root"));
        }
        // Validate referenced roots even when this pass has no observations.
        store.page(&Some(reference.clone()), None, 1)?;
    }
    Ok(root)
}

fn objects(agent_id: &str) -> io::Result<std::path::PathBuf> {
    Ok(chat_read::locations(agent_id)?.1)
}

pub(super) fn pending(state: &ConversationCaptureState, agent_id: &str) -> io::Result<bool> {
    let mut store = Store::new(&objects(agent_id)?);
    Ok(load(&mut store, state.compatibility_claims.as_ref())?
        .pending
        .is_some())
}

fn scope(
    context: &ConversationArchiveContext,
    conversation_id: &str,
    source: &ProviderLogCaptureState,
) -> io::Result<String> {
    Ok(digest(
        &serde_json::to_vec(&serde_json::json!({
            "agent": context.agent_id, "conversation": conversation_id,
            "provider": context.provider, "sessions": context.provider_session_ids,
            "source": context.provider_source_key, "path": source.path,
            "identity": source.native_identity, "policy": source.policy_generation,
            "unknown": source.unknown_before_offset, "disabled": source.disabled_spans,
            "open": source.open_disabled_from,
        }))
        .map_err(io::Error::other)?,
    ))
}

fn qualified(source: &ProviderLogCaptureState) -> bool {
    matches!(source.status.as_str(), "complete" | "pending")
        && source.reason.is_none()
        && source.unknown_before_offset.is_none()
        && source.disabled_spans.is_empty()
        && source.open_disabled_from.is_none()
}

fn stored_scope(
    context: &ConversationArchiveContext,
    conversation_id: &str,
    manifest: Option<&ConversationManifest>,
    source: &ProviderLogCaptureState,
) -> bool {
    manifest.is_some_and(|manifest| {
        manifest.agent_id == context.agent_id
            && manifest.conversation_id == conversation_id
            && manifest.provider == "claude"
            && context.provider == "claude"
            && !context.provider_session_ids.is_empty()
            && manifest.provider_session_ids == context.provider_session_ids
            && manifest.provider_source_key == context.provider_source_key
            && context.provider_source_key.as_deref() == Some(source.provider_source_key.as_str())
    })
}

fn owner_key(context: &ConversationArchiveContext, conversation_id: &str, id: &str) -> String {
    digest(format!("owner:{}:{conversation_id}:{id}", context.agent_id).as_bytes())
}

fn admitted(event: &AgentChatEvent, source: &ProviderLogCaptureState) -> bool {
    let Some((start, end)) = event.metadata["chat_source_start"]
        .as_u64()
        .zip(event.metadata["chat_source_end"].as_u64())
    else {
        return false;
    };
    let epoch = digest(&serde_json::to_vec(&source.native_identity).unwrap_or_default());
    let prefix = format!("source:{}:{epoch}:{start}:", event.session_id);
    let Some(suffix) = event.metadata["chat_source_ref"]
        .as_str()
        .and_then(|value| value.strip_prefix(&prefix))
    else {
        return false;
    };
    let Some((raw_digest, ordinal)) = suffix.split_once(':') else {
        return false;
    };
    start < end
        && end <= source.committed_offset
        && start >= source.unknown_before_offset.unwrap_or(0)
        && source
            .open_disabled_from
            .is_none_or(|disabled| end <= disabled)
        && !source
            .disabled_spans
            .iter()
            .any(|span| start < span.end && end > span.start)
        && event.metadata["chat_source_epoch"].as_str() == Some(epoch.as_str())
        && valid_ref(raw_digest)
        && ordinal == "0"
}

fn eligible(
    old: &AgentChatEvent,
    current: &AgentChatEvent,
    source: &ProviderLogCaptureState,
    sessions: &[String],
) -> bool {
    old.provider == "claude"
        && old.provider == current.provider
        && old.session_id == current.session_id
        && old.kind == AgentChatEventKind::Message
        && old.kind == current.kind
        && old.role == current.role
        && old.metadata["generated"] != true
        && current.metadata["generated"] != true
        && old.metadata["provider_log"] == true
        && current.metadata["provider_log"] == true
        && !old.metadata["chat_source_ref"].is_string()
        && [old, current].into_iter().all(|event| {
            event.metadata["provider_session_id"]
                .as_str()
                .is_none_or(|session| sessions.iter().any(|known| known == session))
        })
        && old.metadata["log_path"]
            .as_str()
            .zip(current.metadata["log_path"].as_str())
            .is_some_and(|(a, b)| a == b)
        && old.metadata["log_path"]
            .as_str()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .is_some_and(|path| path.to_string_lossy() == source.path)
        && old.metadata["request_root_id"]
            .as_str()
            .filter(|uuid| !uuid.is_empty())
            .is_some_and(|uuid| current.metadata["request_root_id"].as_str() == Some(uuid))
        && current.metadata["chat_compatibility_native_uuid"]
            .as_str()
            .is_some_and(|uuid| !uuid.is_empty())
        && admitted(current, source)
}

fn reserve(store: &mut Store, root: &mut Root, owner_key: &str, claim: &Claim) -> io::Result<()> {
    let coordinate_key =
        digest(format!("coordinate:{}:{}", claim.scope, claim.coordinate).as_bytes());
    for key in [owner_key, coordinate_key.as_str()] {
        if let Some(reference) = store.get(&root.owners, key)? {
            let existing: Claim = store.read(&reference)?;
            if existing != *claim {
                return Err(io::Error::other(
                    "conflicting legacy coordinate reservation",
                ));
            }
        }
    }
    let reference = store.put(claim)?;
    for key in [owner_key, coordinate_key.as_str()] {
        root.owners = Some(store.insert(&root.owners, key, &reference)?);
    }
    for key in [owner_key, coordinate_key.as_str()] {
        if store.get(&root.owners, key)?.as_deref() != Some(reference.as_str()) {
            return Err(io::Error::other("unpublished compatibility reservation"));
        }
    }
    root.pending = Some(store.insert(&root.pending, owner_key, &reference)?);
    Ok(())
}

/// Prepare immutable claims using only archive data already loaded by its writer.
/// The returned overlay is private until the caller commits cursor and policy.
pub(super) struct Preparation<'a> {
    pub(super) context: &'a ConversationArchiveContext,
    pub(super) conversation_id: &'a str,
    pub(super) manifest: Option<&'a ConversationManifest>,
    pub(super) archived: &'a [AgentChatEvent],
    pub(super) records: &'a [ConversationNarrativeRecord],
    pub(super) events: &'a [AgentChatEvent],
    pub(super) previous: Option<&'a ProviderLogCaptureState>,
    pub(super) next: &'a ProviderLogCaptureState,
}

/// Persist a bounded pending seek root with the unchanged source cursor.
pub(super) fn prepare(
    input: Preparation<'_>,
    state: &mut ConversationCaptureState,
) -> io::Result<(Vec<AgentChatEvent>, HashMap<String, AgentChatEvent>)> {
    let Preparation {
        context,
        conversation_id,
        manifest,
        archived,
        records,
        events,
        previous,
        next,
    } = input;
    let mut store = Store::writer(&objects(&context.agent_id)?);
    let mut root = load(&mut store, state.compatibility_claims.as_ref())?;
    let mut owners: HashMap<&str, Vec<&AgentChatEvent>> = HashMap::new();
    for event in archived {
        owners.entry(&event.id).or_default().push(event);
    }
    let stored_scope = stored_scope(context, conversation_id, manifest, next);
    let continuity = previous.is_some_and(|previous| {
        previous.provider_source_key == next.provider_source_key
            && previous.path == next.path
            && previous.native_identity == next.native_identity
            && previous.policy_generation == next.policy_generation
            && previous.committed_offset <= next.committed_offset
            && qualified(previous)
            && next.status == "complete"
            && qualified(next)
    });
    let current_scope = scope(context, conversation_id, next)?;
    let mut remaining = Vec::new();
    for event in events {
        let Some(id) = event.metadata["chat_compatibility_raw_id"].as_str() else {
            remaining.push(event.clone());
            continue;
        };
        let Some(matches) = owners.get(id) else {
            remaining.push(event.clone());
            continue;
        };
        if matches.len() != 1 {
            return Err(io::Error::other("nonunique legacy native owner"));
        }
        let old = matches[0];
        if !stored_scope
            || !continuity
            || !eligible(old, event, next, &context.provider_session_ids)
        {
            remaining.push(event.clone());
            continue;
        }
        if records
            .iter()
            .filter(|record| record.event_refs.contains(&old.id))
            .count()
            != 1
        {
            return Err(io::Error::other("nonunique legacy narrative owner"));
        }
        let claim = Claim {
            scope: current_scope.clone(),
            canonical_id: old.id.clone(),
            coordinate: event.metadata["chat_source_ref"]
                .as_str()
                .unwrap()
                .to_string(),
            native_uuid: event.metadata["chat_compatibility_native_uuid"]
                .as_str()
                .unwrap()
                .to_string(),
            request_root_id: event.metadata["request_root_id"]
                .as_str()
                .unwrap()
                .to_string(),
            end: event.metadata["chat_source_end"].as_u64().unwrap(),
        };
        let key = owner_key(context, conversation_id, &old.id);
        if let Some(reference) = store.get(&root.owners, &key)? {
            let old_claim: Claim = store.read(&reference)?;
            if old_claim.scope != claim.scope {
                remaining.push(event.clone());
                continue;
            }
        }
        reserve(&mut store, &mut root, &key, &claim)?;
    }
    let pending = store.page(&root.pending, None, READ_OBJECTS + 1)?;
    if pending.len() > READ_OBJECTS {
        return Err(io::Error::other("compatibility pending budget exhausted"));
    }
    let mut overlay = HashMap::new();
    for (_, reference) in pending {
        let claim: Claim = store.read(&reference)?;
        if !stored_scope || !continuity || claim.scope != current_scope {
            continue;
        }
        let matches = owners
            .get(claim.canonical_id.as_str())
            .ok_or_else(|| io::Error::other("missing reserved native owner"))?;
        if matches.len() != 1 {
            return Err(io::Error::other("nonunique reserved native owner"));
        }
        let mut display = matches[0].clone();
        let prefix = format!(
            "source:{}:{}:",
            context.agent_id,
            digest(&serde_json::to_vec(&next.native_identity).map_err(io::Error::other)?)
        );
        let coordinate = claim
            .coordinate
            .strip_prefix(&prefix)
            .ok_or_else(|| io::Error::other("foreign reserved coordinate"))?;
        let start = coordinate
            .split(':')
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| io::Error::other("invalid reserved extent"))?;
        display.metadata["chat_source_ref"] = serde_json::json!(claim.coordinate);
        display.metadata["chat_source_start"] = serde_json::json!(start);
        display.metadata["chat_source_end"] = serde_json::json!(claim.end);
        display.metadata["chat_source_epoch"] = serde_json::json!(digest(
            &serde_json::to_vec(&next.native_identity).map_err(io::Error::other)?
        ));
        if display.metadata["request_root_id"].as_str() != Some(claim.request_root_id.as_str())
            || !admitted(&display, next)
        {
            return Err(io::Error::other("reserved native proof changed"));
        }
        overlay.insert(display.id.clone(), display);
    }
    if root.pending.is_some() {
        state.compatibility_claims = Some(Checkpoint {
            version: 1,
            root: store.put(&root)?,
        });
        load(&mut store, state.compatibility_claims.as_ref())?;
        write_capture_state(&context.agent_id, state)?;
    }
    Ok((remaining, overlay))
}

/// Reapply immutable completed bindings to already-loaded candidate rows.
/// Pending tracks crash recovery; clearing it must not erase presentation ownership.
pub(super) fn restore_candidate(
    candidate: &mut chat_read::Candidate,
    state: &ConversationCaptureState,
) -> io::Result<()> {
    if state.compatibility_claims.is_none() {
        return Ok(());
    }
    let mut store = Store::writer(&objects(&candidate.context.agent_id)?);
    let root = load(&mut store, state.compatibility_claims.as_ref())?;
    let Some(source) = state.provider_log_sources.iter().find(|source| {
        candidate.context.provider_source_key.as_deref()
            == Some(source.provider_source_key.as_str())
    }) else {
        return Ok(());
    };
    let manifest = read_manifest(
        &conversation_dir(&candidate.context.agent_id, &candidate.conversation_id)?
            .join("manifest.json"),
    )?;
    if !qualified(source)
        || !stored_scope(
            &candidate.context,
            &candidate.conversation_id,
            manifest.as_ref(),
            source,
        )
    {
        return Ok(());
    }
    let current_scope = scope(&candidate.context, &candidate.conversation_id, source)?;
    let mut restored = HashSet::new();
    let mut seeks = super::chat_read_store::SeekCache::default();
    for event in &mut candidate.events {
        if event.metadata["chat_source_ref"].is_string()
            || event.metadata["provider_log"] != true
            || event.metadata["generated"] == true
        {
            continue;
        }
        let key = owner_key(&candidate.context, &candidate.conversation_id, &event.id);
        let Some(reference) = store.get_reusing_nodes(&root.owners, &key, &mut seeks)? else {
            continue;
        };
        let claim: Claim = store.read(&reference)?;
        if claim.scope != current_scope || claim.end > source.committed_offset {
            continue;
        }
        if claim.canonical_id != event.id || !restored.insert(event.id.clone()) {
            return Err(io::Error::other("invalid completed native owner"));
        }
        let coordinate_key =
            digest(format!("coordinate:{}:{}", claim.scope, claim.coordinate).as_bytes());
        if store
            .get_reusing_nodes(&root.owners, &coordinate_key, &mut seeks)?
            .as_deref()
            != Some(reference.as_str())
        {
            return Err(io::Error::other("invalid completed reverse owner"));
        }
        let epoch = digest(&serde_json::to_vec(&source.native_identity).map_err(io::Error::other)?);
        let prefix = format!("source:{}:{epoch}:", candidate.context.agent_id);
        let start = claim
            .coordinate
            .strip_prefix(&prefix)
            .and_then(|value| value.split(':').next())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| io::Error::other("invalid completed coordinate"))?;
        let mut display = event.clone();
        display.metadata["chat_source_ref"] = serde_json::json!(claim.coordinate);
        display.metadata["chat_source_start"] = serde_json::json!(start);
        display.metadata["chat_source_end"] = serde_json::json!(claim.end);
        display.metadata["chat_source_epoch"] = serde_json::json!(epoch);
        display.metadata["chat_compatibility_native_uuid"] = serde_json::json!(claim.native_uuid);
        if display.metadata["request_root_id"].as_str() != Some(claim.request_root_id.as_str())
            || !eligible(
                event,
                &display,
                source,
                &candidate.context.provider_session_ids,
            )
        {
            return Err(io::Error::other("completed native proof changed"));
        }
        *event = display;
    }
    Ok(())
}

pub(super) fn overlay(
    mut events: Vec<AgentChatEvent>,
    replacements: &HashMap<String, AgentChatEvent>,
) -> Vec<AgentChatEvent> {
    for event in &mut events {
        if let Some(display) = replacements.get(&event.id) {
            *event = display.clone();
        }
    }
    events
}

pub(super) fn finish(state: &mut ConversationCaptureState, agent_id: &str) -> io::Result<()> {
    let mut store = Store::writer(&objects(agent_id)?);
    let mut root = load(&mut store, state.compatibility_claims.as_ref())?;
    if root.pending.take().is_some() {
        state.compatibility_claims = Some(Checkpoint {
            version: 1,
            root: store.put(&root)?,
        });
        write_capture_state(agent_id, state)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "compatibility_claims_tests.rs"]
mod tests;
