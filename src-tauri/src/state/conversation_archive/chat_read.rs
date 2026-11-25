//! Legacy projection formats and helpers shared with the archive reader.
//! Normal Chat reads the archive directly. Legacy projection publication is
//! retained for compatibility fixtures.
use std::collections::BTreeSet;
#[cfg(test)]
use std::collections::{BTreeMap, HashMap, VecDeque};
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(test)]
use std::path::Component;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use serde_json::json;
use wardian_core::conversations::write_json_atomic;
use wardian_core::models::chat::AgentChatEvent;
#[cfg(test)]
use wardian_core::models::chat::AgentChatPage;
use wardian_core::paths::agent_conversations_dir;

#[cfg(test)]
use super::chat_read_store::{Dependency, SeekCache, Store};
use super::storage::digest;
#[cfg(test)]
use super::storage::valid_ref;
use super::ConversationArchiveContext;

#[path = "chat_archive_bootstrap.rs"]
mod cold_bootstrap;
#[cfg(test)]
#[path = "chat_read_packed_recent.rs"]
pub(super) mod packed_recent;
#[cfg(test)]
#[path = "chat_preview_bridge.rs"]
mod preview_bridge;
#[cfg(test)]
#[path = "chat_preview_projection.rs"]
mod preview_projection;
#[cfg(test)]
#[path = "chat_retired_view.rs"]
mod retired_view;
#[cfg(test)]
pub(crate) use retired_view::Read as RetiredChatRead;
#[cfg(test)]
#[path = "chat_turn_read.rs"]
mod turn_read;

pub(crate) const PAGE_ROWS: usize = 80;
pub(crate) const PREVIEW_BYTES: usize = 1024;
pub(crate) const BODY_BYTES: usize = 16 * 1024;
pub(crate) const POLICY_BYTES: u64 = 32 * 1024;
pub(crate) const IPC_BYTES: usize = 256 * 1024;
pub(crate) const MAX_REVALIDATED_SOURCE_IDS: usize = 8;
pub(crate) const MAX_REVALIDATED_SOURCE_ID_BYTES: usize = 512;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SourcePolicy {
    pub(crate) agent_id: String,
    pub(crate) source_key: String,
    pub(crate) identity: crate::commands::provider_log_acquisition::ProviderLogNativeIdentity,
    pub(crate) anchor: crate::commands::provider_log_acquisition::ProviderLogContinuityAnchor,
    pub(crate) generation: u64,
    pub(crate) unknown_before: u64,
    pub(crate) disabled: Vec<crate::commands::provider_log_acquisition::ProviderLogDisabledSpan>,
    pub(crate) open_disabled: Option<u64>,
    pub(crate) valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) blocked_reason: Option<String>,
    pub(crate) framed_starts: Vec<u64>,
}

fn policy_path(agent_id: &str) -> io::Result<PathBuf> {
    Ok(locations(agent_id)?
        .0
        .with_file_name("chat-source-policy.json"))
}

pub(super) fn policy_barrier(agent_id: &str) -> io::Result<()> {
    write_json_atomic(&policy_path(agent_id)?, &Option::<SourcePolicy>::None)
}

pub(super) fn publish_policy(
    context: &ConversationArchiveContext,
    state: &crate::commands::provider_log_acquisition::ProviderLogCaptureState,
) -> io::Result<()> {
    let blocked_reason = state
        .reason
        .as_deref()
        .filter(|reason| {
            matches!(
                *reason,
                "provider_log_source_replaced"
                    | "provider_log_truncated"
                    | "provider_log_continuity_mismatch"
            )
        })
        .map(str::to_owned);
    let mut policy = SourcePolicy {
        agent_id: context.agent_id.clone(),
        source_key: state.provider_source_key.clone(),
        identity: state.native_identity.clone(),
        anchor: state.continuity_anchor.clone(),
        generation: state.policy_generation,
        unknown_before: state.unknown_before_offset.unwrap_or(0),
        disabled: state.disabled_spans.clone(),
        open_disabled: state.open_disabled_from,
        valid: blocked_reason.is_none(),
        blocked_reason,
        framed_starts: vec![0],
    };
    let mut file = File::open(&state.path)?;
    if crate::commands::provider_log_acquisition::native_file_identity(&file)?
        != state.native_identity
    {
        policy.valid = false;
        policy.blocked_reason = Some("provider_log_source_replaced".into());
    }
    for offset in std::iter::once(policy.unknown_before)
        .chain(policy.disabled.iter().map(|span| span.end))
        .filter(|offset| *offset > 0)
    {
        file.seek(SeekFrom::Start(offset - 1))?;
        let mut byte = [0];
        if file.read(&mut byte)? == 1 && byte[0] == b'\n' {
            policy.framed_starts.push(offset);
        }
    }
    if serde_json::to_vec(&policy).map_err(io::Error::other)?.len() as u64 > POLICY_BYTES {
        return policy_barrier(&context.agent_id);
    }
    write_json_atomic(&policy_path(&context.agent_id)?, &Some(policy))
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Head {
    pub(crate) agent_id: String,
    pub(crate) conversation_id: String,
    pub(crate) source_key: Option<String>,
    pub(crate) generation: String,
    pub(crate) source_epoch: Option<String>,
    pub(crate) root: Option<String>,
    pub(crate) identities: Option<String>,
    #[serde(default)]
    logical: super::chat_logical_index::Index,
    #[serde(
        default,
        skip_serializing_if = "super::chat_turn_index::Index::is_empty"
    )]
    turns: super::chat_turn_index::Index,
    pub(crate) committed_output_bytes: u64,
    pub(crate) capture_stamp: String,
    pub(crate) progress: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_coverage: Option<CanonicalSourceCoverage>,
    parent: Option<String>,
    changes: Vec<String>,
    recent_start: usize,
    row_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recent_page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap: Option<cold_bootstrap::Progress>,
}

impl Head {
    pub(crate) fn legacy_pack_binding(&self) -> io::Result<String> {
        // Removing this reference avoids a content-hash cycle. All other
        // fields, including historical defaults, use the Head serializer.
        let mut unbound = self.clone();
        unbound.recent_page = None;
        Ok(digest(
            &serde_json::to_vec(&unbound).map_err(io::Error::other)?,
        ))
    }
}

/// Writer-produced native interval coverage, separate from archive byte extent
/// and opaque capture fingerprints. Old checkpoints deliberately have no proof.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct CanonicalSourceCoverage {
    pub(super) source_key: String,
    pub(super) epoch: String,
    pub(super) admission: String,
    pub(super) extent: u64,
    pub(super) normalizer_settled: bool,
}

/// Created inside a successful archive writer, but handed to the projection
/// owner only after that caller's required cursor/policy commit also succeeds.
pub(super) struct Candidate {
    pub(super) context: ConversationArchiveContext,
    pub(super) conversation_id: String,
    pub(super) events: Vec<AgentChatEvent>,
    pub(super) committed_output_bytes: u64,
    #[cfg(test)]
    pub(super) verified_ids: std::collections::HashSet<String>,
    #[cfg(test)]
    pub(super) generated_ids: std::collections::HashSet<String>,
    #[cfg(test)]
    pub(super) generated_input_bindings: HashMap<String, String>,
    #[cfg(test)]
    pub(super) source_epoch: Option<String>,
}

pub(super) fn locations(agent_id: &str) -> io::Result<(PathBuf, PathBuf)> {
    let dir = agent_conversations_dir(agent_id)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .ok_or_else(|| io::Error::other("unsafe chat agent path"))?;
    Ok((
        dir.join("chat-read-head.json"),
        dir.join("chat-read-objects"),
    ))
}

#[cfg(test)]
mod test_support {
    use super::*;

    pub(crate) fn source_index_test_snapshot(
        snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    ) -> io::Result<Option<SourceIndexTestSnapshot>> {
        let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
        let Some(reference) = super::super::chat_source_index::pointer(&context.agent_id)? else {
            return Ok(None);
        };
        let mut source_store = store(&context.agent_id)?;
        let (generation, epoch, progress) =
            super::super::chat_source_index::scope(snapshot, &mut source_store, &reference)?;
        let complete = super::super::chat_source_index::current_complete_scope(
            snapshot,
            &reference,
            &generation,
            &epoch,
        );
        let head: serde_json::Value = source_store.read(&reference)?;
        let logical = &head["logical"];
        let scalars = serde_json::json!({
            "invalidations": head
                .get("invalidations")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "target_extent": head
                .get("target_extent")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "before": head.get("before").cloned().unwrap_or(serde_json::Value::Null),
            "framed": head.get("framed").cloned().unwrap_or(serde_json::Value::Null),
            "replay_settled": head
                .get("replay_settled")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "replay_records_decoded": head
                .get("replay_records_decoded")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "replay_rows_admitted": head
                .get("replay_rows_admitted")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "replay_bytes_read": head
                .get("replay_bytes_read")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "logical_pending": logical
                .get("pending")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "logical_claims": logical
                .get("claims")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "logical_before": logical
                .get("before")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "logical_buckets": logical
                .get("buckets")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "narrative_queue": logical
                .get("narrative_queue")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "narrative_before": logical
                .get("narrative_before")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "narrative_watermark": logical
                .get("narrative_watermark")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        });
        Ok(Some(SourceIndexTestSnapshot {
            generation,
            epoch,
            progress,
            complete,
            scalars,
        }))
    }
}

#[cfg(test)]
pub(crate) use test_support::source_index_test_snapshot;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InputFence {
    pub(crate) conversation_id: String,
    pub(crate) source_epoch: Option<String>,
    pub(super) policy_generation: Option<u64>,
}

pub(crate) fn clip(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// Narrative headers end before a partial word. Body chunk offsets and identity
/// metadata retain their byte-exact clipping rules.
fn narrative_preview(value: &str, limit: usize) -> String {
    let bounded = clip(value, limit);
    if bounded.len() == value.len() {
        return bounded;
    }
    let end = if value[bounded.len()..].starts_with(char::is_whitespace) {
        bounded.len()
    } else {
        bounded.rfind(char::is_whitespace).unwrap_or(0)
    };
    bounded[..end].trim_end().to_owned()
}

fn complete_path(value: &serde_json::Value) -> Option<&str> {
    value.as_str().filter(|path| path.len() <= 256)
}

fn input_preview(input: &serde_json::Value, depth: usize) -> (serde_json::Value, bool) {
    let mut preview = serde_json::Map::new();
    let mut truncated = false;
    let Some(input) = input.as_object() else {
        return (json!({}), true);
    };
    for (key, value) in input {
        let selected = match key.as_str() {
            "file_path" | "filePath" | "AbsolutePath" | "TargetFile" | "path" => {
                complete_path(value).map(|path| json!(path))
            }
            "old_string" | "new_string" | "oldString" | "newString" | "content" | "contents"
            | "CodeContent" => value.as_str().map(|value| {
                truncated |= value.len() > 128;
                json!(clip(value, 128))
            }),
            "edits" if depth == 0 => value.as_array().map(|edits| {
                truncated |= edits.len() > 8;
                json!(edits
                    .iter()
                    .take(8)
                    .map(|edit| {
                        let (preview, partial) = input_preview(edit, depth + 1);
                        truncated |= partial;
                        preview
                    })
                    .collect::<Vec<_>>())
            }),
            "replace_all" => value.as_bool().map(|value| json!(value)),
            _ => None,
        };
        if let Some(value) = selected {
            preview.insert(key.clone(), value);
            // JSON escaping counts too. Never publish an oversized semantic input.
            if serde_json::to_vec(&preview).map_or(true, |bytes| bytes.len() > 2048) {
                preview.remove(key);
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    (serde_json::Value::Object(preview), truncated)
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut characters = text.chars().peekable();
    let mut lines = 0;
    let mut terminated = false;
    while let Some(character) = characters.next() {
        terminated = character == '\r' || character == '\n';
        if terminated {
            lines += 1;
            if character == '\r' && characters.peek() == Some(&'\n') {
                characters.next();
            }
        }
    }
    lines + usize::from(!terminated)
}

fn edit_summary(event: &AgentChatEvent) -> Option<serde_json::Value> {
    let input = event.metadata["tool_input"].as_object()?;
    let mut added = 0;
    let mut removed = 0;
    let mut edited = false;
    let mut count_pair = |input: &serde_json::Map<String, serde_json::Value>| {
        let old = input
            .get("old_string")
            .or_else(|| input.get("oldString"))
            .and_then(|value| value.as_str());
        let new = input
            .get("new_string")
            .or_else(|| input.get("newString"))
            .and_then(|value| value.as_str());
        if (old.is_some() || new.is_some()) && old.unwrap_or_default() != new.unwrap_or_default() {
            edited = true;
            removed += line_count(old.unwrap_or_default());
            added += line_count(new.unwrap_or_default());
        }
    };
    if let Some(edits) = input.get("edits").and_then(|value| value.as_array()) {
        for edit in edits {
            if let Some(edit) = edit.as_object() {
                count_pair(edit);
            }
        }
    } else {
        count_pair(input);
    }
    if edited {
        return Some(json!({"kind": "edit", "added": added, "removed": removed}));
    }
    let name = event.metadata["tool_name"]
        .as_str()
        .or(event.title.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        name.as_str(),
        "write" | "create_file" | "createfile" | "write_file" | "write_to_file"
    ) {
        return None;
    }
    let content = ["content", "contents", "CodeContent"]
        .iter()
        .find_map(|key| input.get(*key).and_then(|value| value.as_str()))?;
    Some(json!({"kind": "write", "added": line_count(content), "removed": 0}))
}

/// Full provider arguments are read only through lazy bodies, never a page DTO.
pub(crate) fn tool_input_body(event: &AgentChatEvent) -> Option<String> {
    if let Some(text) = event.metadata["tool_input_text"].as_str() {
        return Some(text.to_owned());
    }
    if let Some(input) = event
        .metadata
        .get("tool_input")
        .filter(|input| !input.is_null())
    {
        return serde_json::to_string(input).ok();
    }
    event
        .metadata
        .get("files_written")
        .filter(|value| value.as_array().is_some_and(|paths| !paths.is_empty()))
        .and_then(|value| serde_json::to_string(value).ok())
}

/// Native framed details use the same argument/output ordering as canonical bodies.
pub(crate) fn inline_body_text(event: &AgentChatEvent) -> Option<String> {
    match (tool_input_body(event), event.text.as_deref()) {
        (Some(input), Some(text)) if !text.is_empty() => Some(format!("{input}\n\n{text}")),
        (Some(input), _) => Some(input),
        (_, Some(text)) => Some(text.to_owned()),
        _ => None,
    }
}

/// Copy only bounded display fields. Raw payloads and archive records are never
/// serialized into a header or used by the browser to derive unseen rows.
pub(crate) fn header(event: &AgentChatEvent) -> AgentChatEvent {
    let mut metadata = serde_json::Map::new();
    let mut clipped_identity = false;
    for key in [
        "input_origin",
        "input_purpose",
        "request_root_id",
        "causal_ref",
        "context_observation",
        "provider_turn_id",
        "provider_phase",
        "provider_session_id",
        "log_path",
        "source_path",
        "conversation_archive_id",
        "codex_display_text_sha256",
        "codex_display_text_bytes",
        crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256,
        "codex_display_turn_conflict",
        "chat_compatibility_legacy_id",
        "chat_compatibility_source_sequence",
        "chat_legacy_source_sequence",
        "provider_step_source",
        "raw_type",
        "generated",
        "provider_log",
        "chat_source_ref",
        "chat_source_start",
        "chat_source_end",
        "chat_source_epoch",
        "chat_source_admission",
        "chat_recent_key",
        "chat_recent_version",
        "chat_recent_native_key",
        "chat_recent_native_version",
        "mock_watch_alias",
        "chat_identity_resolution",
        "tool_name",
        "call_id",
        "tool_call_id",
        "tool_use_id",
        "chat_turn_adapter",
        "chat_turn_membership_complete",
        "chat_turn_membership_partial",
        "chat_turn_after_closed",
        "chat_turn_membership_contradiction",
        "chat_turn_membership_contradicted_root",
        "chat_turn_membership_expected_provider_turn",
    ] {
        if let Some(value) = event.metadata.get(key) {
            if let Some(value) = value.as_str() {
                let identity = matches!(
                    key,
                    "request_root_id"
                        | "provider_turn_id"
                        | "call_id"
                        | "tool_call_id"
                        | "tool_use_id"
                        | "chat_source_ref"
                        | "conversation_archive_id"
                        | "chat_turn_membership_contradicted_root"
                        | "chat_turn_membership_expected_provider_turn"
                );
                let limit = if identity { 512 } else { 256 };
                clipped_identity |= identity && value.len() > limit;
                metadata.insert(key.into(), json!(clip(value, limit)));
            } else if value.is_boolean() || value.is_number() {
                metadata.insert(key.into(), value.clone());
            }
        }
    }
    if clipped_identity {
        // A clipped identity cannot derive the future canonical item key.
        metadata.insert("chat_turn_membership_partial".into(), json!(true));
    }
    if let Some(path) = complete_path(&event.metadata["file_path"]) {
        metadata.insert("file_path".into(), json!(path));
    }
    let mut truncated = false;
    if let Some(input) = event.metadata["tool_input_text"].as_str() {
        let mut preview = clip(input, PREVIEW_BYTES);
        if input.len() > PREVIEW_BYTES {
            // A partial patch header could invent a different file path.
            preview.truncate(preview.rfind('\n').map_or(0, |newline| newline + 1));
        }
        metadata.insert("tool_input_text".into(), json!(preview));
        truncated |= input.len() > PREVIEW_BYTES;
    }
    if event.metadata["tool_input"].is_object() {
        let (preview, partial) = input_preview(&event.metadata["tool_input"], 0);
        metadata.insert("tool_input".into(), preview);
        truncated |= partial;
        if let Some(summary) = edit_summary(event) {
            metadata.insert("chat_edit_summary".into(), summary);
        }
    }
    if let Some(paths) = event.metadata["files_written"].as_array() {
        let selected = paths
            .iter()
            .take(16)
            .filter_map(complete_path)
            .collect::<Vec<_>>();
        if selected.len() != paths.len() {
            metadata.insert("chat_files_written_truncated".into(), json!(true));
        }
        metadata.insert("files_written".into(), json!(selected));
    }
    if truncated {
        metadata.insert("chat_tool_input_truncated".into(), json!(true));
    }
    if matches!(
        event.kind,
        wardian_core::models::chat::AgentChatEventKind::ToolCall
            | wardian_core::models::chat::AgentChatEventKind::ToolResult
            | wardian_core::models::chat::AgentChatEventKind::Approval
    ) {
        let no_artifacts = ["text_artifact_refs", "artifact_refs"]
            .iter()
            .all(|key| event.metadata[*key].as_array().is_none_or(Vec::is_empty))
            && event.metadata["archive_record"]["artifact_refs"]
                .as_array()
                .is_none_or(Vec::is_empty);
        let inline_complete = event
            .text
            .as_ref()
            .is_some_and(|text| text.len() <= PREVIEW_BYTES)
            && event.metadata["text_excerpt"].is_null()
            && (event.metadata["archive_record"]["excerpt"].is_null()
                || !event.metadata["archive_record"]["text"].is_null())
            && no_artifacts
            && !truncated;
        metadata.insert("chat_step_inline_complete".into(), json!(inline_complete));
    }
    AgentChatEvent {
        id: event.id.clone(),
        session_id: event.session_id.clone(),
        provider: clip(&event.provider, 32),
        kind: event.kind.clone(),
        role: event.role.clone(),
        text: event
            .text
            .as_deref()
            .or_else(|| event.metadata["text_excerpt"].as_str())
            .or_else(|| event.metadata["archive_record"]["excerpt"].as_str())
            .map(|text| narrative_preview(text, PREVIEW_BYTES)),
        title: event.title.as_deref().map(|v| clip(v, 256)),
        status: event.status.clone(),
        turn_id: event.turn_id.as_deref().map(|v| clip(v, 256)),
        source: event.source.as_deref().map(|v| clip(v, 128)),
        command: event.command.as_deref().map(|v| clip(v, 256)),
        exit_code: event.exit_code,
        path: event
            .path
            .as_deref()
            .filter(|path| path.len() <= 256)
            .map(str::to_owned),
        language: event.language.as_deref().map(|v| clip(v, 32)),
        created_at: event.created_at.as_deref().map(|v| clip(v, 64)),
        sequence: event.sequence,
        metadata: serde_json::Value::Object(metadata),
    }
}

pub(crate) fn validate_revalidation_request(
    agent_id: &str,
    cursor: Option<&str>,
    revision: Option<&str>,
    detail_ref: Option<&str>,
    source_ids: Option<Vec<String>>,
) -> io::Result<Option<Vec<String>>> {
    let Some(source_ids) = source_ids else {
        return Ok(None);
    };
    if source_ids.is_empty()
        || source_ids.len() > MAX_REVALIDATED_SOURCE_IDS
        || cursor.is_some()
        || detail_ref.is_some()
        || revision.is_none()
    {
        return Err(io::Error::other("invalid chat revalidation request"));
    }
    let mut unique = Vec::with_capacity(source_ids.len().min(MAX_REVALIDATED_SOURCE_IDS));
    let mut seen = BTreeSet::new();
    for source_id in source_ids {
        if source_id.len() > MAX_REVALIDATED_SOURCE_ID_BYTES
            || !super::chat_source_index::valid_source_id(agent_id, &source_id)
        {
            return Err(io::Error::other("invalid chat revalidation request"));
        }
        if seen.insert(source_id.clone()) {
            unique.push(source_id);
        }
    }
    Ok(Some(unique))
}

#[cfg(test)]
pub(crate) use tests::{read, read_with_revalidation};

#[cfg(test)]
#[path = "chat_read_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "chat_turn_publication_tests.rs"]
mod turn_presentation_tests;

#[cfg(test)]
#[path = "chat_preview_capacity_tests.rs"]
mod preview_capacity_tests;

#[cfg(test)]
include!("chat_read_legacy_fixture.inc.rs");
