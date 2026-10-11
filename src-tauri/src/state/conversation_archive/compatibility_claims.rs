//! Capture-only reservations for historically persisted provider-log owners.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_owner_digest: Option<String>,
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
#[derive(Clone, Copy)]
pub(super) struct Preparation<'a> {
    pub(super) context: &'a ConversationArchiveContext,
    pub(super) conversation_id: &'a str,
    pub(super) manifest: Option<&'a ConversationManifest>,
    pub(super) archived: &'a [AgentChatEvent],
    pub(super) records: &'a [ConversationNarrativeRecord],
    pub(super) events: &'a [AgentChatEvent],
    pub(super) previous: Option<&'a ProviderLogCaptureState>,
    pub(super) next: &'a ProviderLogCaptureState,
    pub(super) source_proof:
        Option<&'a crate::commands::provider_log_acquisition::ProviderLogSourceProof>,
}

fn codex_capture_scope_matches(input: &Preparation<'_>) -> bool {
    input.context.provider == "codex"
        && !input.context.provider_session_ids.is_empty()
        && input.manifest.is_some_and(|manifest| {
            manifest.provider == "codex"
                && manifest.agent_id == input.context.agent_id
                && manifest.conversation_id == input.conversation_id
                && manifest.provider_source_key == input.context.provider_source_key
                && manifest.provider_session_ids == input.context.provider_session_ids
        })
        && input.previous.is_some_and(|previous| {
            previous.path == input.next.path
                && previous.provider_source_key == input.next.provider_source_key
                && previous.native_identity == input.next.native_identity
                && previous.policy_generation == input.next.policy_generation
                && previous.committed_offset <= input.next.committed_offset
        })
        && input.context.provider_source_key.as_deref()
            == Some(input.next.provider_source_key.as_str())
}

fn codex_scope_matches(input: &Preparation<'_>) -> bool {
    codex_capture_scope_matches(input)
        && input.previous.is_some_and(|previous| {
            previous.unknown_before_offset.is_none()
                && previous.disabled_spans.is_empty()
                && previous.open_disabled_from.is_none()
                && matches!(previous.status.as_str(), "complete" | "pending")
                && matches!(
                    previous.reason.as_deref(),
                    None | Some("provider_log_waiting_for_request_root")
                )
        })
        && qualified(input.next)
}

fn codex_owner_matches(
    old: &AgentChatEvent,
    current: &AgentChatEvent,
    input: &Preparation<'_>,
) -> bool {
    let canonical_role = |event: &AgentChatEvent| {
        let mut event = event.clone();
        provenance::canonicalize_role(&mut event);
        event.role
    };
    old.provider == "codex"
        && current.provider == "codex"
        && old.session_id == input.context.agent_id
        && old.session_id == current.session_id
        && old.kind == current.kind
        && canonical_role(old) == canonical_role(current)
        && old.source == current.source
        && old.turn_id == current.turn_id
        && old.text == current.text
        && old.sequence.is_some()
        && old.sequence == current.sequence
        && old.metadata["provider_log"] == true
        && current.metadata["provider_log"] == true
        && old.metadata["generated"] != true
        && current.metadata["generated"] != true
        && crate::commands::chat::archive_identity::requires_provider_log_row_identity(current)
        // Legacy IDs hashed the declared path. Canonical paths independently
        // prove native ownership below; substituting their bytes changes an
        // existing ID on Windows (including the verbatim path prefix).
        && old.metadata["log_path"] == current.metadata["log_path"]
        && current.metadata["log_path"].as_str().is_some_and(|path| {
            event_identity_ids(old).contains(
                &crate::commands::chat::archive_identity::legacy_provider_log_event_id(
                    current, std::path::Path::new(path),
                ).as_str(),
            )
        })
        && [old, current].into_iter().all(|event| {
            event.metadata["log_path"]
                .as_str()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|path| path.to_string_lossy() == input.next.path)
                && event.metadata["provider_session_id"]
                    .as_str()
                    .is_none_or(|session| {
                        input
                            .context
                            .provider_session_ids
                            .iter()
                            .any(|known| known == session)
                    })
        })
        && [
            "provider_turn_id",
            "provider_event_id",
            "input_origin",
            "raw_type",
            "tool_name",
            "tool_input",
            "tool_input_text",
        ]
        .iter()
        .all(|key| old.metadata.get(*key) == current.metadata.get(*key))
        && admitted(current, input.next)
}

/// Bind immutable Codex reservations to semantic owner fields, independent of
/// canonical role normalization and the source frame added during migration.
fn codex_owner_digest(event: &AgentChatEvent) -> io::Result<String> {
    let mut canonical = event.clone();
    provenance::canonicalize_role(&mut canonical);
    Ok(digest(
        &serde_json::to_vec(&serde_json::json!({
            "id": event.id, "session": event.session_id, "provider": event.provider,
            "kind": event.kind, "role": canonical.role, "sequence": event.sequence,
            "source": event.source, "turn": event.turn_id, "created": event.created_at,
            "title": event.title, "command": event.command, "text": event.text,
            "log_path": event.metadata.get("log_path"),
            "provider_session": event.metadata.get("provider_session_id"),
            "provider_turn": event.metadata.get("provider_turn_id"),
            "provider_event": event.metadata.get("provider_event_id"),
            "request_root": event.metadata.get("request_root_id"),
            "input_origin": event.metadata.get("input_origin"),
            "raw_type": event.metadata.get("raw_type"),
            "tool_name": event.metadata.get("tool_name"),
            "tool_input": event.metadata.get("tool_input"),
            "tool_input_text": event.metadata.get("tool_input_text"),
        }))
        .map_err(io::Error::other)?,
    ))
}

fn codex_progress_qualified(source: &ProviderLogCaptureState) -> bool {
    matches!(source.status.as_str(), "complete" | "pending")
        && matches!(
            source.reason.as_deref(),
            None | Some(
                "provider_log_waiting_for_request_root"
                    | "provider_log_batch_limit"
                    | "provider_log_partial_record"
            )
        )
        && source.unknown_before_offset.is_none()
        && source.disabled_spans.is_empty()
        && source.open_disabled_from.is_none()
}

/// Cursor publication follows the durable frame upgrade. Recover that exact
/// owner without asking a later acquisition to re-observe already-consumed bytes.
/// An uncommitted cursor or unframed owner still needs the original sealed row.
fn committed_codex_owner(
    input: &Preparation<'_>,
    store: &mut Store,
    root: &Root,
    reservation: (&str, &str),
    claim: &Claim,
    old: &AgentChatEvent,
) -> io::Result<bool> {
    let (pending_key, reference) = reservation;
    let Some(previous) = input.previous else {
        return Ok(false);
    };
    if claim.end > previous.committed_offset || !old.metadata["chat_source_ref"].is_string() {
        return Ok(false);
    }
    let coordinate_key =
        digest(format!("coordinate:{}:{}", claim.scope, claim.coordinate).as_bytes());
    if !codex_capture_scope_matches(input)
        || !codex_progress_qualified(previous)
        || !codex_progress_qualified(input.next)
        || claim.scope != scope(input.context, input.conversation_id, previous)?
        || claim.scope != scope(input.context, input.conversation_id, input.next)?
        || pending_key != owner_key(input.context, input.conversation_id, &claim.canonical_id)
        || store.get(&root.owners, pending_key)?.as_deref() != Some(reference)
        || store.get(&root.owners, &coordinate_key)?.as_deref() != Some(reference)
        || old.id != claim.canonical_id
        || old.provider != "codex"
        || old.session_id != input.context.agent_id
        || old.sequence != claim.codex_sequence
        || old.metadata["provider_log"] != true
        || old.metadata["generated"] == true
        || old.metadata["chat_source_ref"].as_str() != Some(claim.coordinate.as_str())
        || old.metadata["chat_source_end"].as_u64() != Some(claim.end)
        || old.metadata["provider_log_row_offset"].as_u64().is_none()
        || old.metadata["provider_log_row_offset"].as_u64()
            != old.metadata["chat_source_start"].as_u64()
        || !admitted(old, previous)
        || !claim.native_uuid.is_empty()
        || !claim.request_root_id.is_empty()
        || claim.codex_owner_digest.as_deref() != Some(codex_owner_digest(old)?.as_str())
        || !old.metadata["log_path"]
            .as_str()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .is_some_and(|path| path.to_string_lossy() == previous.path)
        || old.metadata["provider_session_id"]
            .as_str()
            .is_some_and(|session| {
                !input
                    .context
                    .provider_session_ids
                    .iter()
                    .any(|known| known == session)
            })
        || input
            .archived
            .iter()
            .filter(|event| {
                event.metadata["chat_source_ref"].as_str() == Some(claim.coordinate.as_str())
            })
            .count()
            != 1
        || input
            .records
            .iter()
            .filter(|record| record.event_refs.contains(&claim.canonical_id))
            .count()
            != 1
    {
        return Err(io::Error::other(
            "committed Codex reservation owner changed",
        ));
    }
    Ok(true)
}

fn codex_upgraded_owner(
    old: &AgentChatEvent,
    current: &AgentChatEvent,
) -> io::Result<AgentChatEvent> {
    let mut display = old.clone();
    for key in [
        "chat_source_ref",
        "chat_source_start",
        "chat_source_end",
        "chat_source_epoch",
        "provider_log_row_offset",
    ] {
        display.metadata[key] = current.metadata[key].clone();
    }
    let mut upgraded = vec![display];
    provenance::refresh_events(&mut upgraded, std::slice::from_ref(current))?;
    if !provenance::same_observation(&upgraded[0], current) {
        return Err(io::Error::other("reserved Codex ownership changed"));
    }
    Ok(upgraded.remove(0))
}

/// Reserve a historical Codex owner only through a sealed acquisition receipt.
/// The shared alias matcher never treats an alias as a physical source proof.
fn prepare_codex_claims(
    input: &Preparation<'_>,
    store: &mut Store,
    root: &mut Root,
    current_scope: &str,
) -> io::Result<()> {
    if input.context.provider != "codex" {
        return Ok(());
    }
    for event in input.events {
        let aliases = event_identity_ids(event);
        let matches = input
            .archived
            .iter()
            .filter(|old| {
                old.id != event.id
                    && aliases.contains(&old.id.as_str())
                    && !old.metadata["chat_source_ref"].is_string()
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            continue;
        }
        let Some(proof) = input.source_proof else {
            return Err(io::Error::other(
                "Codex legacy coordinate needs source recovery proof",
            ));
        };
        if !codex_scope_matches(input)
            || !proof.matches_capture(input.previous, input.next)?
            || !proof.proves(event)
            || matches.len() != 1
            || input
                .events
                .iter()
                .filter(|candidate| {
                    candidate.metadata["chat_source_ref"] == event.metadata["chat_source_ref"]
                })
                .count()
                != 1
        {
            return Err(io::Error::other(
                "unqualified Codex legacy coordinate owner",
            ));
        }
        let old = matches[0];
        if !codex_owner_matches(old, event, input)
            || ["chat_source_start", "chat_source_end", "chat_source_epoch"]
                .iter()
                .any(|key| old.metadata.get(*key).is_some())
            || input
                .archived
                .iter()
                .filter(|candidate| candidate.id == old.id)
                .count()
                != 1
            || input
                .records
                .iter()
                .filter(|record| record.event_refs.contains(&old.id))
                .count()
                != 1
        {
            return Err(io::Error::other(
                "conflicting Codex historical coordinate owner",
            ));
        }
        let claim = Claim {
            scope: current_scope.to_string(),
            canonical_id: old.id.clone(),
            coordinate: event.metadata["chat_source_ref"]
                .as_str()
                .unwrap()
                .to_string(),
            native_uuid: String::new(),
            request_root_id: String::new(),
            end: event.metadata["chat_source_end"].as_u64().unwrap(),
            codex_sequence: old.sequence,
            codex_owner_digest: Some(codex_owner_digest(&codex_upgraded_owner(old, event)?)?),
        };
        reserve(
            store,
            root,
            &owner_key(input.context, input.conversation_id, &old.id),
            &claim,
        )?;
    }
    Ok(())
}

/// Persist a bounded pending seek root with the unchanged source cursor.
pub(super) fn prepare(
    input: Preparation<'_>,
    state: &mut ConversationCaptureState,
) -> io::Result<(Vec<AgentChatEvent>, HashMap<String, AgentChatEvent>)> {
    let mut store = Store::writer(&objects(&input.context.agent_id)?);
    let mut root = load(&mut store, state.compatibility_claims.as_ref())?;
    let current_scope = scope(input.context, input.conversation_id, input.next)?;
    prepare_codex_claims(&input, &mut store, &mut root, &current_scope)?;
    let codex_scope = codex_scope_matches(&input);
    let Preparation {
        context,
        conversation_id,
        manifest,
        archived,
        records,
        events,
        previous,
        next,
        source_proof,
    } = input;
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
            codex_sequence: None,
            codex_owner_digest: None,
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
    for (key, reference) in pending {
        let claim: Claim = store.read(&reference)?;
        if let Some(sequence) = claim.codex_sequence {
            let old = owners
                .get(claim.canonical_id.as_str())
                .filter(|matches| matches.len() == 1)
                .ok_or_else(|| io::Error::other("nonunique reserved Codex owner"))?[0];
            if committed_codex_owner(&input, &mut store, &root, (&key, &reference), &claim, old)? {
                overlay.insert(claim.canonical_id, old.clone());
                continue;
            }
            let Some(proof) = source_proof else {
                return Err(io::Error::other("missing reserved Codex source proof"));
            };
            if !codex_scope
                || claim.scope != current_scope
                || !proof.matches_capture(previous, next)?
            {
                return Err(io::Error::other("reserved Codex source changed"));
            }
            let current = events
                .iter()
                .filter(|event| {
                    event.sequence == Some(sequence)
                        && event.metadata["chat_source_ref"].as_str()
                            == Some(claim.coordinate.as_str())
                        && event.metadata["chat_source_end"].as_u64() == Some(claim.end)
                        && proof.proves(event)
                })
                .collect::<Vec<_>>();
            if current.len() != 1
                || old.sequence != Some(sequence)
                || old.metadata["generated"] == true
            {
                return Err(io::Error::other("reserved Codex row changed"));
            }
            if !codex_owner_matches(old, current[0], &input)
                || old.metadata["chat_source_ref"]
                    .as_str()
                    .is_some_and(|coordinate| coordinate != claim.coordinate)
                || old.metadata["chat_source_epoch"]
                    .as_str()
                    .is_some_and(|epoch| {
                        current[0].metadata["chat_source_epoch"].as_str() != Some(epoch)
                    })
                || old.metadata["provider_log_row_offset"]
                    .as_u64()
                    .is_some_and(|offset| {
                        current[0].metadata["provider_log_row_offset"].as_u64() != Some(offset)
                    })
            {
                return Err(io::Error::other(
                    "conflicting reserved Codex source coordinate",
                ));
            }
            overlay.insert(claim.canonical_id, codex_upgraded_owner(old, current[0])?);
            continue;
        }
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
