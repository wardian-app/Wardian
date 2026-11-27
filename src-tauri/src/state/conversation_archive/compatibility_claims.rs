//! Bind historical provider-log rows to sealed capture receipts.
//! Legacy CAS reservation helpers are compiled only by compatibility fixtures.
#[cfg(test)]
use super::chat_read_store::{Store, READ_OBJECTS};
use super::storage::{digest, valid_ref};
use super::*;
use crate::commands::provider_log_acquisition::ProviderLogCaptureState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Checkpoint {
    version: u8,
    root: String,
}

fn sealed_interval_qualified(source: &ProviderLogCaptureState) -> bool {
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

/// Bind a provider batch to the archived conversation and its capture receipt.
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
        && input.previous.is_some_and(sealed_interval_qualified)
        && sealed_interval_qualified(input.next)
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

/// Upgrade historical Codex frames using only the sealed batch and the exact
/// archived owner. The caller publishes these frames before advancing capture.
pub(super) fn codex_archive_upgrades(
    input: &Preparation<'_>,
) -> io::Result<Vec<(usize, AgentChatEvent)>> {
    if input.context.provider != "codex" {
        return Ok(Vec::new());
    }
    let mut upgrades = Vec::new();
    for event in input.events {
        let aliases = event_identity_ids(event);
        let matches = input
            .archived
            .iter()
            .enumerate()
            .filter(|(_, old)| {
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
        let (index, old) = matches[0];
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
        if upgrades.iter().any(|(claimed, _)| *claimed == index)
            || input
                .archived
                .iter()
                .any(|saved| saved.metadata["chat_source_ref"] == event.metadata["chat_source_ref"])
        {
            return Err(io::Error::other("conflicting Codex archive upgrade"));
        }
        upgrades.push((index, codex_upgraded_owner(old, event)?));
    }
    Ok(upgrades)
}

#[cfg(test)]
#[path = "compatibility_claims_tests.rs"]
mod tests;

/// An exact formerly saved Claude raw-frame identity can satisfy capture
/// without changing that event or its separately owned legacy alias narrative.
pub(super) fn already_archived_claude_rows(input: &Preparation<'_>) -> io::Result<HashSet<String>> {
    let mut covered = HashSet::new();
    let mut saved_ids = HashSet::new();
    for event in input.events {
        let Some(raw_id) = event.metadata["chat_compatibility_raw_id"].as_str() else {
            continue;
        };
        let mut owners = input.archived.iter().filter(|old| old.id == raw_id);
        let Some(old) = owners.next() else {
            continue;
        };
        let proof = input.source_proof.ok_or_else(|| {
            io::Error::other("Claude raw archive owner needs source recovery proof")
        })?;
        let proved = event.metadata["log_path"]
            .as_str()
            .is_some_and(|path| proof.proves_claude_raw_owner(event, std::path::Path::new(path)));
        if owners.next().is_some()
            || !stored_scope(
                input.context,
                input.conversation_id,
                input.manifest,
                input.next,
            )
            || !input.previous.is_some_and(|previous| {
                previous.provider_source_key == input.next.provider_source_key
                    && previous.path == input.next.path
                    && previous.native_identity == input.next.native_identity
                    && previous.policy_generation == input.next.policy_generation
                    && previous.committed_offset <= input.next.committed_offset
                    && sealed_interval_qualified(previous)
            })
            || !sealed_interval_qualified(input.next)
            || !proved
            || !proof.matches_capture(input.previous, input.next)?
            || !eligible(old, event, input.next, &input.context.provider_session_ids)
            || old.text != event.text
            || input
                .records
                .iter()
                .filter(|record| record.event_refs.contains(&old.id))
                .count()
                != 1
            || input
                .archived
                .iter()
                .any(|saved| saved.metadata["chat_source_ref"] == event.metadata["chat_source_ref"])
            || !saved_ids.insert(raw_id)
            || !covered.insert(event.id.clone())
        {
            return Err(io::Error::other(
                "Claude raw archive owner could not be verified",
            ));
        }
    }
    Ok(covered)
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

#[cfg(test)]
include!("compatibility_claims_legacy_fixture.inc.rs");
