//! The capture owner builds immutable display indexes in bounded checkpoints.
//! Normal Chat never enters the archive gate or reads raw archive JSONL.
use std::collections::{BTreeSet, HashMap, VecDeque};
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;
use wardian_core::conversations::write_json_atomic;
use wardian_core::models::chat::{AgentChatEvent, AgentChatPage};
use wardian_core::paths::agent_conversations_dir;

use super::chat_read_store::{digest, valid_ref, Dependency, Store};
use super::ConversationArchiveContext;

#[path = "chat_archive_bootstrap.rs"]
mod cold_bootstrap;

pub(crate) const PAGE_ROWS: usize = 80;
pub(crate) const PREVIEW_BYTES: usize = 1024;
pub(crate) const BODY_BYTES: usize = 16 * 1024;
const HEAD_BYTES: u64 = 4096;
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
    let mut policy = SourcePolicy {
        agent_id: context.agent_id.clone(),
        source_key: state.provider_source_key.clone(),
        identity: state.native_identity.clone(),
        anchor: state.continuity_anchor.clone(),
        generation: state.policy_generation,
        unknown_before: state.unknown_before_offset.unwrap_or(0),
        disabled: state.disabled_spans.clone(),
        open_disabled: state.open_disabled_from,
        valid: !matches!(
            state.reason.as_deref(),
            Some(
                "provider_log_source_replaced"
                    | "provider_log_truncated"
                    | "provider_log_continuity_mismatch"
            )
        ),
        framed_starts: vec![0],
    };
    let mut file = File::open(&state.path)?;
    if crate::commands::provider_log_acquisition::native_file_identity(&file)?
        != state.native_identity
    {
        policy.valid = false;
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

pub(crate) fn source_policy(context: &ConversationArchiveContext) -> io::Result<SourcePolicy> {
    let mut file = File::open(policy_path(&context.agent_id)?)?;
    let extent = file.metadata()?.len();
    if extent > POLICY_BYTES {
        return Err(io::Error::other("oversized source policy"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(extent).read_to_end(&mut bytes)?;
    let policy: Option<SourcePolicy> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let policy = policy.ok_or_else(|| io::Error::other("source policy transition pending"))?;
    if bytes.len() as u64 != extent
        || !policy.valid
        || policy.agent_id != context.agent_id
        || Some(policy.source_key.as_str()) != context.provider_source_key.as_deref()
        || policy.disabled.len() > 256
        || policy.framed_starts.len() > 258
        || policy.anchor.len > 4096
    {
        return Err(io::Error::other("source policy unavailable"));
    }
    Ok(policy)
}

impl SourcePolicy {
    pub(crate) fn admission_digest(&self) -> io::Result<String> {
        // Cursor/anchor progress does not alter which source bytes are private.
        Ok(digest(
            &serde_json::to_vec(&(
                &self.agent_id,
                &self.source_key,
                &self.identity,
                self.generation,
                self.unknown_before,
                &self.disabled,
                self.open_disabled,
                self.valid,
                &self.framed_starts,
            ))
            .map_err(io::Error::other)?,
        ))
    }
    pub(crate) fn same_admission(&self, other: &Self) -> bool {
        self.agent_id == other.agent_id
            && self.source_key == other.source_key
            && self.identity == other.identity
            && self.generation == other.generation
            && self.unknown_before == other.unknown_before
            && self.disabled == other.disabled
            && self.open_disabled == other.open_disabled
            && self.valid == other.valid
            && self.framed_starts == other.framed_starts
    }
    pub(crate) fn admits(&self, start: u64, end: u64) -> bool {
        self.valid
            && start >= self.unknown_before
            && start < end
            && self.open_disabled.is_none_or(|disabled| end <= disabled)
            && !self
                .disabled
                .iter()
                .any(|span| start < span.end && end > span.start)
    }

    /// Choose a single admitted interval. Disabled bytes are not read or decoded.
    pub(crate) fn recent_interval(&self, extent: u64) -> Option<(u64, u64)> {
        let mut end = self.open_disabled.unwrap_or(extent).min(extent);
        for span in self.disabled.iter().rev() {
            if span.start >= end {
                continue;
            }
            if span.end < end {
                return (span.end.max(self.unknown_before) < end)
                    .then_some((span.end.max(self.unknown_before), end));
            }
            end = span.start;
        }
        (self.unknown_before < end).then_some((self.unknown_before, end))
    }
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
    pub(crate) committed_output_bytes: u64,
    pub(crate) capture_stamp: String,
    pub(crate) progress: String,
    parent: Option<String>,
    changes: Vec<String>,
    recent_start: usize,
    row_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap: Option<cold_bootstrap::Progress>,
}

impl Head {
    fn checkpoint_dependencies(&self) -> Vec<Dependency> {
        let mut roots = self
            .root
            .iter()
            .chain(self.identities.iter())
            .map(|root| Dependency::Node(root.clone()))
            .collect::<Vec<_>>();
        roots.extend(
            self.logical
                .node_roots()
                .map(|root| Dependency::Node(root.to_owned())),
        );
        if let Some(bootstrap) = &self.bootstrap {
            roots.extend(
                bootstrap
                    .node_roots()
                    .map(|root| Dependency::Node(root.to_owned())),
            );
        }
        roots.extend(
            self.changes
                .iter()
                .chain(self.parent.iter())
                .map(|reference| Dependency::Object(reference.clone())),
        );
        roots
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Row {
    agent_id: String,
    conversation_id: String,
    key: String,
    event: AgentChatEvent,
    body: Option<Body>,
    #[serde(default)]
    relation: Option<super::chat_logical_index::Relation>,
    #[serde(default)]
    removed_display_ids: Vec<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Body {
    binding: String,
    root: Option<String>,
    bytes: u64,
    complete: bool,
}

/// Created inside a successful archive writer, but handed to the projection
/// owner only after that caller's required cursor/policy commit also succeeds.
pub(super) struct Candidate {
    pub(super) context: ConversationArchiveContext,
    pub(super) conversation_id: String,
    pub(super) events: Vec<AgentChatEvent>,
    pub(super) committed_output_bytes: u64,
    pub(super) verified_ids: std::collections::HashSet<String>,
    pub(super) generated_ids: std::collections::HashSet<String>,
    pub(super) generated_input_bindings: HashMap<String, String>,
    pub(super) source_epoch: Option<String>,
}

struct Work {
    candidate: Candidate,
    head: Head,
    todo: VecDeque<usize>,
    signatures: Vec<(String, String)>,
    published: Option<String>,
    bodies: VecDeque<BodyWork>,
    source_proof: Option<super::chat_logical_index::SourceProof>,
}

struct BodyWork {
    row: Row,
    source: BodySource,
    offset: u64,
}

enum BodySource {
    Text(String),
    Combined(Vec<BodySource>),
    File {
        file: File,
        length: u64,
        modified: Option<std::time::SystemTime>,
    },
}

impl BodySource {
    fn binding(&self) -> io::Result<String> {
        match self {
            Self::Text(text) => Ok(digest(text.as_bytes())),
            Self::Combined(parts) => Ok(digest(
                &serde_json::to_vec(
                    &parts
                        .iter()
                        .map(Self::binding)
                        .collect::<io::Result<Vec<_>>>()?,
                )
                .map_err(io::Error::other)?,
            )),
            Self::File {
                file,
                length,
                modified,
            } => Ok(digest(
                format!(
                    "{:?}:{length}:{modified:?}",
                    crate::commands::provider_log_acquisition::native_file_identity(file)?
                )
                .as_bytes(),
            )),
        }
    }

    fn length(&self) -> io::Result<u64> {
        match self {
            Self::Text(text) => Ok(text.len() as u64),
            Self::File { length, .. } => Ok(*length),
            Self::Combined(parts) => parts.iter().try_fold(0u64, |total, part| {
                total
                    .checked_add(part.length()?)
                    .ok_or_else(|| io::Error::other("chat body length overflow"))
            }),
        }
    }

    // A checkpoint reads one segment, so metadata plus an artifact stays within
    // the same per-chunk budget and retains the artifact's held-handle checks.
    fn chunk(&mut self, mut offset: u64) -> io::Result<Vec<u8>> {
        match self {
            Self::Text(text) => {
                let tail = text
                    .get(offset as usize..)
                    .ok_or_else(|| io::Error::other("invalid chat body offset"))?;
                Ok(clip(tail, BODY_BYTES).into_bytes())
            }
            Self::Combined(parts) => {
                for part in parts {
                    let length = part.length()?;
                    if offset < length {
                        return part.chunk(offset);
                    }
                    offset -= length;
                }
                Ok(Vec::new())
            }
            Self::File {
                file,
                length,
                modified,
            } => {
                let metadata = file.metadata()?;
                if metadata.len() != *length
                    || metadata.modified().ok() != *modified
                    || offset > *length
                {
                    return Err(io::Error::other(
                        "chat body source changed during checkpoint",
                    ));
                }
                file.seek(SeekFrom::Start(offset))?;
                let mut bytes = Vec::new();
                (&mut *file)
                    .take((*length - offset).min(BODY_BYTES as u64))
                    .read_to_end(&mut bytes)?;
                let metadata = file.metadata()?;
                if metadata.len() != *length || metadata.modified().ok() != *modified {
                    return Err(io::Error::other(
                        "chat body source changed during checkpoint",
                    ));
                }
                match std::str::from_utf8(&bytes) {
                    Ok(_) => {}
                    Err(error) if error.error_len().is_none() => {
                        bytes.truncate(error.valid_up_to())
                    }
                    Err(_) => return Err(io::Error::other("invalid UTF-8 chat body")),
                }
                Ok(bytes)
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct ProjectionOwner {
    work: Mutex<HashMap<String, Work>>,
    #[cfg(test)]
    fail_checkpoint_stage: std::sync::atomic::AtomicU8,
}

impl std::fmt::Debug for ProjectionOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectionOwner").finish_non_exhaustive()
    }
}

#[cfg(test)]
impl ProjectionOwner {
    fn checkpoint_fault(&self, stage: u8) -> io::Result<()> {
        use std::sync::atomic::Ordering;
        if self
            .fail_checkpoint_stage
            .compare_exchange(stage, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Err(io::Error::other("injected chat checkpoint failure"));
        }
        Ok(())
    }
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

pub(crate) fn head(agent_id: &str) -> io::Result<Option<String>> {
    let (path, _) = locations(agent_id)?;
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > HEAD_BYTES {
        return Err(io::Error::other("oversized chat head"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(HEAD_BYTES).read_to_end(&mut bytes)?;
    let reference: Option<String> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let Some(reference) = reference else {
        return Ok(None);
    };
    if !valid_ref(&reference) {
        return Err(io::Error::other("invalid chat head"));
    }
    Ok(Some(reference))
}

/// A missing raw file cannot turn a previously committed display extent into
/// a new empty history. Other conversations do not impose that prior extent.
pub(super) fn committed_output_extent(
    agent_id: &str,
    conversation_id: &str,
) -> io::Result<Option<u64>> {
    let Some(reference) = head(agent_id)? else {
        return Ok(None);
    };
    let head: Head = store(agent_id)?.read(&reference)?;
    if head.agent_id != agent_id {
        return Err(io::Error::other("foreign chat head"));
    }
    Ok((head.conversation_id == conversation_id).then_some(head.committed_output_bytes))
}

/// Bind unresolved provider names only to actual accepted input records, without
/// turning generated archive identity into native-provider correspondence.
pub(super) fn generated_input_bindings(
    records: &[wardian_core::conversations::ConversationNarrativeRecord],
    conversation_id: &str,
) -> HashMap<String, String> {
    use wardian_core::conversations::{ConversationInputOrigin, ConversationRecordKind};
    records
        .iter()
        .filter(|record| {
            record.kind == ConversationRecordKind::Message
                && record.role.as_deref() == Some("user")
                && matches!(
                    record.input_origin,
                    Some(ConversationInputOrigin::HumanInput | ConversationInputOrigin::AgentInput)
                )
                && record.input_purpose.as_deref() == Some("request")
                && record
                    .request_root_id
                    .as_deref()
                    .is_some_and(|root| !root.is_empty())
        })
        .flat_map(|record| {
            let expected_id = format!("generated:{conversation_id}:{}", record.seq);
            let binding = digest(
                serde_json::to_string(&json!([
                    record.seq,
                    record.input_origin,
                    record.input_purpose,
                    record.request_root_id,
                ]))
                .unwrap()
                .as_bytes(),
            );
            record
                .event_refs
                .iter()
                .filter(move |id| id.as_str() == expected_id)
                .map(move |id| (id.clone(), binding.clone()))
        })
        .collect()
}

/// Keep the provider unresolved until native correspondence is independently
/// verified; the canonical accepted-input binding supplies eligibility only.
pub(super) fn is_owned_unknown_input(candidate: &Candidate, event: &AgentChatEvent) -> bool {
    let record = &event.metadata["archive_record"];
    event.session_id == candidate.context.agent_id
        && event.provider == "unknown"
        && event.metadata["generated"] == true
        && event.kind == wardian_core::models::chat::AgentChatEventKind::Message
        && event.role == Some(wardian_core::models::chat::AgentChatRole::User)
        && event
            .id
            .starts_with(&format!("generated:{}:", candidate.conversation_id))
        && candidate.generated_ids.contains(&event.id)
        && record["kind"] == "message"
        && record["role"] == "user"
        && event.metadata["input_origin"] == record["input_origin"]
        && event.metadata["input_purpose"] == record["input_purpose"]
        && candidate
            .generated_input_bindings
            .get(&event.id)
            .is_some_and(|binding| {
                *binding
                    == digest(
                        serde_json::to_string(&json!([
                            record["seq"],
                            record["input_origin"],
                            record["input_purpose"],
                            record["request_root_id"],
                        ]))
                        .unwrap()
                        .as_bytes(),
                    )
            })
}

fn store(agent_id: &str) -> io::Result<Store> {
    Ok(Store::new(&locations(agent_id)?.1))
}

pub(crate) fn advance_source_index(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
) -> io::Result<bool> {
    super::chat_source_index::advance(snapshot)
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InputFence {
    pub(crate) conversation_id: String,
    pub(crate) source_epoch: Option<String>,
    policy_generation: Option<u64>,
}

/// A bounded published scope, never an archive scan or owner-lock acquisition.
pub(crate) fn input_fence(
    context: &ConversationArchiveContext,
    source_path: Option<&Path>,
) -> io::Result<Option<InputFence>> {
    let Some(reference) = head(&context.agent_id)? else {
        return Ok(None);
    };
    let published: Head = store(&context.agent_id)?.read(&reference)?;
    if published.agent_id != context.agent_id || published.source_key != context.provider_source_key
    {
        return Ok(None);
    }
    let mut policy_generation = None;
    if matches!(context.provider.as_str(), "codex" | "claude" | "pi") {
        let policy = source_policy(context)?;
        let Some(path) = source_path else {
            return Ok(None);
        };
        if crate::commands::provider_log_acquisition::native_file_identity(&File::open(path)?)?
            != policy.identity
        {
            return Ok(None);
        }
        let epoch = digest(&serde_json::to_vec(&policy.identity).map_err(io::Error::other)?);
        if published.source_epoch.as_deref() != Some(epoch.as_str())
            || policy.open_disabled.is_some()
        {
            return Ok(None);
        }
        policy_generation = Some(policy.generation);
    }
    Ok(Some(InputFence {
        conversation_id: published.conversation_id,
        source_epoch: published.source_epoch,
        policy_generation,
    }))
}

pub(crate) fn clip(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
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
        "chat_identity_resolution",
        "tool_name",
    ] {
        if let Some(value) = event.metadata.get(key) {
            if let Some(value) = value.as_str() {
                metadata.insert(key.into(), json!(clip(value, 256)));
            } else if value.is_boolean() || value.is_number() {
                metadata.insert(key.into(), value.clone());
            }
        }
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
            .map(|text| clip(text, PREVIEW_BYTES)),
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

fn body_source(candidate: &Candidate, event: &AgentChatEvent) -> io::Result<Option<BodySource>> {
    let input = tool_input_body(event);
    let mut parts = Vec::new();
    if let Some(input) = input.as_ref() {
        parts.push(BodySource::Text(input.clone()));
    }
    if let Some(body) = output_body_source(candidate, event, input.is_some())? {
        if !parts.is_empty() {
            parts.push(BodySource::Text("\n\n".into()));
        }
        parts.push(body);
    }
    Ok(match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(BodySource::Combined(parts)),
    })
}

fn output_body_source(
    candidate: &Candidate,
    event: &AgentChatEvent,
    include_short: bool,
) -> io::Result<Option<BodySource>> {
    if let Some(text) = event.text.as_ref().filter(|text| {
        text.len() > PREVIEW_BYTES
            || include_short
                && !text.is_empty()
                && event.metadata["text_artifact_refs"].is_null()
                && event.metadata["archive_record"]["artifact_refs"].is_null()
    }) {
        return Ok(Some(BodySource::Text(text.clone())));
    }
    let Some(reference) = event.metadata["text_artifact_refs"]
        .as_array()
        .or_else(|| event.metadata["archive_record"]["artifact_refs"].as_array())
        .and_then(|refs| refs.first())
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };
    let path = Path::new(reference);
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        || path.components().count() == 0
        || reference.contains(':')
        || reference
            .split(['/', '\\'])
            .any(|part| matches!(part, "." | ".."))
    {
        return Err(io::Error::other("unsafe chat body artifact"));
    }
    let directory =
        super::storage::conversation_dir(&candidate.context.agent_id, &candidate.conversation_id)?;
    // The archive materializer stores basenames; older envelopes may include
    // the artifacts directory. Both forms stay within this owned conversation.
    let path = if path.components().count() == 1 {
        directory.join("artifacts").join(path)
    } else if path.starts_with("artifacts") {
        directory.join(path)
    } else {
        return Err(io::Error::other("unsafe chat body artifact"));
    };
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    Ok(Some(BodySource::File {
        file,
        length: metadata.len(),
        modified: metadata.modified().ok(),
    }))
}

impl ProjectionOwner {
    /// Restore saved history in fixed checkpoints without hydrating the archive
    /// writer. The caller holds the policy and per-agent archive gates.
    pub(super) fn bootstrap_saved(&self, context: &ConversationArchiveContext) -> io::Result<bool> {
        let works = self
            .work
            .lock()
            .map_err(|_| io::Error::other("chat projection owner poisoned"))?;
        if works.contains_key(&context.agent_id) {
            return Ok(false);
        }
        cold_bootstrap::advance(context)
    }

    pub(super) fn committed(&self, candidate: Candidate, capture_stamp: String) -> io::Result<()> {
        let source_proof = super::chat_logical_index::source_proof(&candidate.context)?;
        let signatures = candidate
            .events
            .iter()
            .map(|event| {
                let display = header(event);
                let body = event.metadata["text_artifact_refs"]
                    .as_array()
                    .or_else(|| event.metadata["archive_record"]["artifact_refs"].as_array())
                    .and_then(|refs| refs.first());
                // Artifact identity/extent changes must invalidate a body even if
                // its path and short preview remain unchanged. No payload is read.
                let body_binding = body_source(&candidate, event)?
                    .map(|source| source.binding())
                    .transpose()?
                    .unwrap_or_default();
                let signature = digest(
                    format!(
                        "{}:{body:?}:{body_binding}",
                        serde_json::to_string(&display).unwrap_or_default()
                    )
                    .as_bytes(),
                );
                Ok((event.id.clone(), signature))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut head = Head {
            agent_id: candidate.context.agent_id.clone(),
            conversation_id: candidate.conversation_id.clone(),
            source_key: candidate.context.provider_source_key.clone(),
            generation: uuid::Uuid::new_v4().to_string(),
            source_epoch: candidate.source_epoch.clone(),
            root: None,
            identities: None,
            logical: Default::default(),
            committed_output_bytes: candidate.committed_output_bytes,
            capture_stamp,
            progress: "indexing".into(),
            parent: None,
            changes: Vec::new(),
            recent_start: candidate.events.len().saturating_sub(PAGE_ROWS),
            row_count: candidate.events.len(),
            bootstrap: None,
        };
        let agent_id = candidate.context.agent_id.clone();
        let mut todo = (0..candidate.events.len()).rev().collect::<VecDeque<_>>();
        let mut bodies = VecDeque::new();
        let mut published = None;
        let mut works = self
            .work
            .lock()
            .map_err(|_| io::Error::other("chat projection owner poisoned"))?;
        if let Some(old) = works.remove(&agent_id) {
            let compatible = old.head.conversation_id == head.conversation_id
                && old.head.source_key == head.source_key
                && old.head.source_epoch == head.source_epoch
                && old.source_proof.as_ref().map(|s| &s.admission)
                    == source_proof.as_ref().map(|s| &s.admission)
                && signatures.len() >= old.signatures.len()
                && old
                    .signatures
                    .iter()
                    .zip(&signatures)
                    .all(|(old, next)| old.0 == next.0);
            if compatible {
                let changed = signatures
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| {
                        (old.signatures.get(index) != Some(value)).then_some(index)
                    })
                    .collect::<std::collections::HashSet<_>>();
                todo = changed
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .collect();
                todo.make_contiguous().sort_unstable_by(|a, b| b.cmp(a));
                todo.extend(
                    old.todo
                        .into_iter()
                        .filter(|index| !changed.contains(index)),
                );
                bodies = old
                    .bodies
                    .into_iter()
                    .filter(|job| {
                        job.row
                            .key
                            .parse::<usize>()
                            .is_ok_and(|index| !changed.contains(&index))
                    })
                    .collect();
                head.root = old.head.root;
                head.identities = old.head.identities;
                head.logical = old.head.logical;
                head.generation = old.head.generation;
                published = old.published;
            }
        }
        head.logical.defer_narratives();
        works.insert(
            agent_id.clone(),
            Work {
                candidate,
                head,
                todo,
                signatures,
                published,
                bodies,
                source_proof,
            },
        );
        drop(works);
        self.advance(&agent_id)?;
        Ok(())
    }

    /// At most 80 headers and 64 KiB of body source per checkpoint. Only the
    /// background capture owner calls this; reads never contend on this mutex.
    pub(crate) fn advance(&self, agent_id: &str) -> io::Result<bool> {
        let mut work_map = self
            .work
            .lock()
            .map_err(|_| io::Error::other("chat projection owner poisoned"))?;
        let Some(work) = work_map.get_mut(agent_id) else {
            return Ok(false);
        };
        // Only small immutable roots and this checkpoint's row/body updates are
        // staged. Candidate payloads and the remaining queues stay in place.
        let mut head = work.head.clone();
        let proof_update = if work.todo.is_empty() {
            let proof = super::chat_logical_index::source_proof(&work.candidate.context)?;
            (work.source_proof.as_ref().map(|s| &s.admission)
                == proof.as_ref().map(|s| &s.admission))
            .then_some(proof)
        } else {
            None
        };
        let source_proof = proof_update.as_ref().unwrap_or(&work.source_proof);
        if proof_update.is_some() {
            head.logical
                .qualify_legacy(source_proof.as_ref().is_some_and(|s| s.complete_prefix));
            head.logical
                .qualify_sequences(source_proof.as_ref().is_some_and(|s| s.sequence_trusted));
        }
        if work.todo.is_empty()
            && work.bodies.is_empty()
            && work.candidate.events.is_empty()
            && !head.logical.pending()
            && !head.logical.narrative_pending()
        {
            return Ok(false);
        }
        let (head_path, objects) = locations(agent_id)?;
        let mut store = Store::checkpoint_writer(&objects);
        let limit = if work.bodies.is_empty() {
            PAGE_ROWS
        } else {
            PAGE_ROWS - 4
        };
        let mut changes = HashMap::new();
        let mut new_bodies = VecDeque::new();
        let mut processed_rows = 0;
        for (ordinal, &index) in work.todo.iter().take(limit).enumerate() {
            if store.objects >= 6000 {
                break;
            }
            let event = &work.candidate.events[index];
            if event.session_id != agent_id
                || (event.provider != work.candidate.context.provider
                    && !is_owned_unknown_input(&work.candidate, event))
                || event.id.len() > 256
            {
                return Err(io::Error::other("foreign chat projection event"));
            }
            let mut row = Row {
                agent_id: agent_id.into(),
                conversation_id: head.conversation_id.clone(),
                key: format!("{:020}", index),
                event: header(event),
                body: None,
                relation: None,
                removed_display_ids: Vec::new(),
            };
            row.event.sequence = Some(index as u64 + 1);
            if let Some(source) = body_source(&work.candidate, event)? {
                row.body = Some(Body {
                    binding: source.binding()?,
                    ..Body::default()
                });
                new_bodies.push_back(BodyWork {
                    row: row.clone(),
                    source,
                    offset: 0,
                });
            }
            let reference = insert_row(&mut store, &mut head, &row)?;
            changes.insert(row.event.id.clone(), reference);
            if let Some(proof) = source_proof {
                let admitted = event.metadata["chat_source_start"]
                    .as_u64()
                    .zip(event.metadata["chat_source_end"].as_u64())
                    .is_none_or(|(start, end)| proof.policy.admits(start, end));
                if admitted {
                    let updates = head.logical.observe(
                        &mut store,
                        &proof.admission(&work.candidate.context, &head.conversation_id),
                        event,
                        &row.key,
                    )?;
                    apply_logical_updates(&mut store, &mut head, updates, &mut changes)?;
                }
            }
            processed_rows = ordinal + 1;
            #[cfg(test)]
            self.checkpoint_fault(1)?;
        }
        let old_body_count = work.bodies.len();
        let body_count = old_body_count + new_bodies.len();
        let mut body_steps: Vec<(usize, Row, u64, bool)> = Vec::new();
        let mut repeated_bodies = VecDeque::new();
        let mut next_body = 0;
        for _ in 0..4 {
            let index = if next_body < body_count {
                let index = next_body;
                next_body += 1;
                index
            } else if let Some(index) = repeated_bodies.pop_front() {
                index
            } else {
                break;
            };
            let job = if index < old_body_count {
                &mut work.bodies[index]
            } else {
                &mut new_bodies[index - old_body_count]
            };
            let (mut row, offset) = body_steps
                .iter()
                .rev()
                .find(|step| step.0 == index)
                .map(|step| (step.1.clone(), step.2))
                .unwrap_or_else(|| (job.row.clone(), job.offset));
            // Held files seek to the staged offset on every read, including a
            // retry. Reading never consumes the durable body job or its offset.
            let bytes = job.source.chunk(offset)?;
            let length = job.source.length()?;
            if bytes.is_empty() && offset < length {
                return Err(io::Error::other("chat body unexpected EOF"));
            }
            let body = row.body.as_mut().expect("body job has descriptor");
            let reference = store.put_bytes(&bytes)?;
            #[cfg(test)]
            self.checkpoint_fault(2)?;
            body.root = Some(store.insert(&body.root, &format!("{:020}", offset), &reference)?);
            let offset = offset + bytes.len() as u64;
            body.bytes = offset;
            body.complete = offset == length;
            let complete = body.complete;
            if let Some(reference) = store.get(&head.root, &row.key)? {
                let current: Row = store.read(&reference)?;
                row.relation = current.relation;
                row.removed_display_ids = current.removed_display_ids;
            }
            let reference = insert_row(&mut store, &mut head, &row)?;
            changes.insert(row.event.id.clone(), reference);
            body_steps.push((index, row, offset, complete));
            if !complete {
                repeated_bodies.push_back(index);
            }
        }
        if processed_rows == work.todo.len() {
            head.logical
                .qualify_legacy(source_proof.as_ref().is_some_and(|s| s.complete_prefix));
            head.logical
                .qualify_sequences(source_proof.as_ref().is_some_and(|s| s.sequence_trusted));
            if let Some(proof) = source_proof
                .as_ref()
                .filter(|proof| proof.narrative_complete)
            {
                head.logical.qualify_narratives(proof.watermark);
                if !store.checkpoint_due() {
                    let updates = head.logical.activate_narratives(&mut store, 2)?;
                    apply_logical_updates(&mut store, &mut head, updates, &mut changes)?;
                }
            }
            if !store.checkpoint_due() {
                let updates = head.logical.activate(&mut store, 2)?;
                apply_logical_updates(&mut store, &mut head, updates, &mut changes)?;
            }
        }
        let pending = processed_rows < work.todo.len()
            || body_steps.iter().filter(|step| step.3).count() < body_count
            || head.logical.pending()
            || head.logical.narrative_pending();
        head.progress = if pending { "indexing" } else { "ready" }.into();
        head.parent = work.published.clone();
        if changes.len() > PAGE_ROWS {
            head.parent = None;
        }
        head.changes = changes.into_values().take(PAGE_ROWS).collect();
        #[cfg(test)]
        self.checkpoint_fault(4)?;
        store.finish_checkpoint(&head.checkpoint_dependencies())?;
        let reference = store.put(&head)?;
        // LAST fallible publication: failed writes leave every queued job and
        // the previous roots available to retry the same unpublished changes.
        #[cfg(test)]
        self.checkpoint_fault(3)?;
        write_json_atomic(&head_path, &reference)?;
        work.head = head;
        work.published = Some(reference);
        if let Some(proof) = proof_update {
            work.source_proof = proof;
        }
        for _ in 0..processed_rows {
            work.todo.pop_front();
        }
        work.bodies.extend(new_bodies);
        for (_, row, offset, complete) in body_steps {
            let mut job = work.bodies.pop_front().expect("staged body job exists");
            debug_assert_eq!(job.row.key, row.key);
            job.row = row;
            job.offset = offset;
            if !complete {
                work.bodies.push_back(job);
            }
        }
        if !pending {
            work.candidate.events.clear();
            work.candidate.verified_ids.clear();
            work.candidate.generated_ids.clear();
        }
        Ok(pending)
    }

    pub(crate) fn invalidate(&self, agent_id: &str) -> io::Result<()> {
        self.work
            .lock()
            .map_err(|_| io::Error::other("chat projection owner poisoned"))?
            .remove(agent_id);
        cold_bootstrap::retire(agent_id)
    }

    /// Try the projection owner before any generated archive write. This seam
    /// never hydrates or scans legacy history and never waits for capture.
    pub(super) fn commit_input(
        &self,
        context: &ConversationArchiveContext,
        conversation_id: &str,
        fence: &InputFence,
        write: impl FnOnce() -> io::Result<(AgentChatEvent, u64)>,
    ) -> io::Result<Option<wardian_core::models::chat::ChatInputReceipt>> {
        let mut works = match self.work.try_lock() {
            Ok(works) => works,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(_) => return Err(io::Error::other("chat projection owner poisoned")),
        };
        let Some(work) = works.get_mut(&context.agent_id) else {
            return Ok(None);
        };
        if work.head.conversation_id != conversation_id
            || work.head.source_key != context.provider_source_key
            || fence.conversation_id != conversation_id
            || fence.source_epoch != work.head.source_epoch
        {
            return Ok(None);
        }
        let (event, extent) = write()?;
        let (_, objects) = locations(&context.agent_id)?;
        let mut store = Store::checkpoint_writer(&objects);
        let mut head = work.head.clone();
        let mut row = Row {
            agent_id: context.agent_id.clone(),
            conversation_id: conversation_id.into(),
            key: format!("{:020}", head.row_count),
            event: header(&event),
            body: None,
            relation: None,
            removed_display_ids: Vec::new(),
        };
        if let Some(text) = event
            .text
            .as_ref()
            .filter(|text| text.len() > PREVIEW_BYTES)
        {
            let mut body = Body {
                binding: digest(text.as_bytes()),
                ..Body::default()
            };
            let mut offset = 0;
            while offset < text.len() {
                let chunk = clip(&text[offset..], BODY_BYTES);
                let reference = store.put_bytes(chunk.as_bytes())?;
                body.root = Some(store.insert(&body.root, &format!("{offset:020}"), &reference)?);
                offset += chunk.len();
            }
            body.bytes = offset as u64;
            body.complete = true;
            row.body = Some(body);
        }
        let reference = insert_row(&mut store, &mut head, &row)?;
        head.parent = work.published.clone();
        head.changes = vec![reference];
        head.row_count += 1;
        head.recent_start = head.row_count.saturating_sub(PAGE_ROWS);
        head.committed_output_bytes = extent;
        #[cfg(test)]
        self.checkpoint_fault(4)?;
        store.finish_checkpoint(&head.checkpoint_dependencies())?;
        let reference = store.put(&head)?;
        #[cfg(test)]
        self.checkpoint_fault(3)?;
        write_json_atomic(&locations(&context.agent_id)?.0, &reference)?;
        work.head = head;
        work.published = Some(reference);
        work.signatures.push((event.id.clone(), String::new()));
        if !work.candidate.events.is_empty() {
            work.candidate.events.push(event.clone());
        }
        Ok(Some(wardian_core::models::chat::ChatInputReceipt {
            chat_event_id: event.id,
            chat_agent_id: context.agent_id.clone(),
            chat_conversation_id: conversation_id.into(),
            chat_source_epoch: work.head.source_epoch.clone(),
        }))
    }
}

fn insert_row(store: &mut Store, head: &mut Head, row: &Row) -> io::Result<String> {
    let dependencies = row
        .body
        .as_ref()
        .and_then(|body| body.root.as_ref())
        .map(|root| Dependency::Node(root.clone()))
        .into_iter()
        .collect();
    let reference = store.put_linked(row, dependencies)?;
    head.root = Some(store.insert(&head.root, &row.key, &reference)?);
    head.identities = Some(store.insert(
        &head.identities,
        &digest(row.event.id.as_bytes()),
        &reference,
    )?);
    // Only the acquisition owner's exact source-coordinate proof is a bridge.
    // Legacy text-derived aliases are deliberately excluded from this index.
    if let Some(source_ref) = row.event.metadata["chat_source_ref"].as_str() {
        head.identities =
            Some(store.insert(&head.identities, &digest(source_ref.as_bytes()), &reference)?);
    }
    Ok(reference)
}

fn stable_commentary_relation(
    store: &mut Store,
    head: &Head,
    mut relation: Option<super::chat_logical_index::Relation>,
) -> io::Result<Option<super::chat_logical_index::Relation>> {
    let Some(current) = relation
        .as_ref()
        .filter(|relation| relation.members.len() == 2)
    else {
        return Ok(relation);
    };
    let mut stream_id = None;
    let mut native_item = false;
    for member_id in &current.members {
        let Some(reference) = store.get(&head.identities, &digest(member_id.as_bytes()))? else {
            return Ok(relation);
        };
        let row: Row = store.read(&reference)?;
        let event = &row.event;
        if row.agent_id != head.agent_id
            || row.conversation_id != head.conversation_id
            || event.id.as_str() != member_id.as_str()
            || event.metadata["chat_source_ref"].as_str() != Some(member_id.as_str())
            || event.provider != "codex"
            || event.kind != wardian_core::models::chat::AgentChatEventKind::Message
            || event.role.as_ref() != Some(&wardian_core::models::chat::AgentChatRole::Assistant)
            || event.metadata["provider_log"] != true
            || event.metadata["generated"] == true
            || event.metadata["provider_phase"].as_str() != Some("commentary")
        {
            return Ok(relation);
        }
        match (event.source.as_deref(), event.metadata["raw_type"].as_str()) {
            (Some("event_msg"), Some("agent_message" | "assistant_message"))
                if event.turn_id.is_none() && stream_id.is_none() =>
            {
                stream_id = Some(event.id.clone());
            }
            (Some("response_item"), Some("message"))
                if event
                    .turn_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty())
                    && !native_item =>
            {
                native_item = true;
            }
            _ => return Ok(relation),
        }
    }
    if let Some(stream_id) = stream_id.filter(|_| native_item) {
        if let Some(relation) = relation.as_mut() {
            relation.id = stream_id.clone();
            relation.prefer_member_first(&stream_id);
        }
    }
    Ok(relation)
}

fn apply_logical_updates(
    store: &mut Store,
    head: &mut Head,
    updates: Vec<(String, Option<super::chat_logical_index::Relation>)>,
    changes: &mut HashMap<String, String>,
) -> io::Result<()> {
    for (key, relation) in updates {
        let relation = stable_commentary_relation(store, head, relation)?;
        let Some(reference) = store.get(&head.root, &key)? else {
            continue;
        };
        let mut row: Row = store.read(&reference)?;
        if row.agent_id != head.agent_id || row.conversation_id != head.conversation_id {
            return Err(io::Error::other("foreign logical row"));
        }
        if row.relation == relation {
            continue;
        }
        if let Some(old) = &row.relation {
            if relation.as_ref().is_none_or(|new| new.id != old.id)
                && !row.removed_display_ids.contains(&old.id)
            {
                row.removed_display_ids.push(old.id.clone());
                row.removed_display_ids.truncate(4);
            }
        }
        row.relation = relation;
        let reference = insert_row(store, head, &row)?;
        changes.insert(row.event.id.clone(), reference);
    }
    Ok(())
}

pub(super) fn alias(
    store: &mut Store,
    head: &Head,
    source_ref: &str,
) -> io::Result<Option<AgentChatEvent>> {
    let Some(reference) = store.get(&head.identities, &digest(source_ref.as_bytes()))? else {
        return Ok(None);
    };
    let row: Row = store.read(&reference)?;
    if row.agent_id != head.agent_id
        || row.conversation_id != head.conversation_id
        || row.event.metadata["chat_source_ref"].as_str() != Some(source_ref)
    {
        return Err(io::Error::other("foreign chat alias"));
    }
    Ok(Some(display_event(row, &reference)))
}

fn display_event(mut row: Row, reference: &str) -> AgentChatEvent {
    if let Some(relation) = row.relation {
        row.event.metadata["chat_display_physical_id"] = json!(row.event.id);
        row.event.id = relation.id;
        row.event.metadata["chat_display_member_ids"] = json!(relation.members);
    }
    if !row.removed_display_ids.is_empty() {
        row.event.metadata["chat_display_removed_ids"] = json!(row.removed_display_ids);
    }
    row.event.metadata["chat_page_key"] = json!(row.key);
    if let Some(body) = &row.body {
        row.event.metadata["chat_body_binding"] = json!(body.binding);
        row.event.metadata["chat_body_pending"] =
            json!(!body.complete && row.event.metadata["chat_body_unavailable"] != true);
        if body.bytes > 0 {
            row.event.metadata["chat_detail_ref"] = json!(format!("{reference}:0"));
        }
    }
    row.event
}

pub(super) fn page(
    store: &mut Store,
    head_ref: &str,
    head: &Head,
    before: Option<&str>,
) -> io::Result<(Vec<AgentChatEvent>, Option<String>)> {
    let entries = store.page(&head.root, before, PAGE_ROWS + 1)?;
    let has_older = entries.len() > PAGE_ROWS;
    let mut events = Vec::new();
    let mut last = None;
    for (key, reference) in entries.into_iter().take(PAGE_ROWS) {
        let row: Row = store.read(&reference)?;
        if row.agent_id != head.agent_id || row.conversation_id != head.conversation_id {
            return Err(io::Error::other("foreign chat row"));
        }
        last = Some(key);
        let mut event = display_event(row, &reference);
        if let Some(detail) = event.metadata["chat_detail_ref"].as_str() {
            event.metadata["chat_detail_ref"] = json!(format!("{head_ref}:{detail}"));
        }
        events.push(event);
    }
    events.reverse();
    let next = if has_older || head.progress == "indexing" {
        last.as_deref()
            .or(before)
            .map(|key| format!("{head_ref}:{key}"))
    } else {
        None
    };
    Ok((events, next))
}

fn changes_since(
    store: &mut Store,
    reference: &str,
    head: &Head,
    previous: &str,
) -> io::Result<Option<Vec<AgentChatEvent>>> {
    let mut reference = reference.to_string();
    let mut current = head.clone();
    let mut rows = HashMap::new();
    for _ in 0..8 {
        if reference == previous {
            return Ok(Some(rows.into_values().collect()));
        }
        if current.generation != head.generation || current.agent_id != head.agent_id {
            return Ok(None);
        }
        for changed in &current.changes {
            if store.objects > 300 {
                return Ok(None);
            }
            let row: Row = store.read(changed)?;
            if row.agent_id != head.agent_id || row.conversation_id != head.conversation_id {
                return Err(io::Error::other("foreign chat delta"));
            }
            if !rows.contains_key(&row.event.id) {
                let historical =
                    row.key.parse::<usize>().map_err(io::Error::other)? < head.recent_start;
                let mut event = display_event(row, changed);
                event.metadata["chat_older_header"] = json!(historical);
                if let Some(detail_ref) = event.metadata["chat_detail_ref"].as_str() {
                    event.metadata["chat_detail_ref"] = json!(format!("{reference}:{detail_ref}"));
                }
                rows.insert(event.id.clone(), event);
                if rows.len() > PAGE_ROWS {
                    return Ok(None);
                }
            }
        }
        let Some(parent) = current.parent else {
            return Ok(None);
        };
        reference = parent;
        current = store.read(&reference)?;
    }
    Ok(None)
}

pub(super) fn detail(
    store: &mut Store,
    head: &Head,
    reference: &str,
    offset: u64,
) -> io::Result<wardian_core::models::chat::AgentChatDetail> {
    let row: Row = store.read(reference)?;
    if row.agent_id != head.agent_id
        || row.conversation_id != head.conversation_id
        || store.get(&head.root, &row.key)?.as_deref() != Some(reference)
    {
        return Err(io::Error::other("stale chat detail"));
    }
    let body = row
        .body
        .ok_or_else(|| io::Error::other("chat body unavailable"))?;
    let chunk = store
        .get(&body.root, &format!("{offset:020}"))?
        .ok_or_else(|| io::Error::other("chat body checkpoint pending"))?;
    let text =
        String::from_utf8(store.read_bytes(&chunk, BODY_BYTES)?).map_err(io::Error::other)?;
    let end = offset + text.len() as u64;
    Ok(wardian_core::models::chat::AgentChatDetail {
        event_id: row.relation.map_or(row.event.id, |relation| relation.id),
        text,
        next: (end < body.bytes
            || !body.complete && row.event.metadata["chat_body_unavailable"] != true)
            .then(|| format!("{reference}:{end}")),
        complete: body.complete && end == body.bytes,
    })
}

pub(crate) fn finish(mut page: AgentChatPage) -> io::Result<AgentChatPage> {
    for event in &mut page.events {
        if let Some(metadata) = event.metadata.as_object_mut() {
            metadata.remove(crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256);
        }
    }
    if serde_json::to_vec(&page).map_err(io::Error::other)?.len() > IPC_BYTES {
        if page.revalidated_ids.is_some() {
            page.events.clear();
            page.aliases.clear();
            page.removed_ids.clear();
            page.detail = None;
            page.next_before = None;
            page.revalidated_ids = None;
            page.progress = "response_budget".into();
            page.reset = true;
            page.unchanged = false;
            return Ok(page);
        }
        // A truncated repair response cannot acknowledge a strict subset of the
        // requested observations as though the whole batch were committed.
        let events = std::mem::take(&mut page.events);
        let base = serde_json::to_vec(&page).map_err(io::Error::other)?.len();
        let mut bytes = base;
        for event in events.into_iter().rev() {
            let size = serde_json::to_vec(&event).map_err(io::Error::other)?.len() + 1;
            if bytes.saturating_add(size) > IPC_BYTES {
                break;
            }
            bytes += size;
            page.events.push(event);
        }
        page.events.reverse();
        if let Some(reference) = page
            .revision
            .split('.')
            .next()
            .filter(|reference| valid_ref(reference))
        {
            // Trimming previews must not skip the omitted indexed headers.
            let before = page
                .events
                .iter()
                .filter_map(|event| event.metadata["chat_page_key"].as_str())
                .min()
                .unwrap_or("~");
            page.next_before = Some(format!("{reference}:{before}"));
        }
        page.progress = "response_budget".into();
        page.unchanged = false;
        if serde_json::to_vec(&page).map_err(io::Error::other)?.len() > IPC_BYTES {
            page.events.clear();
            page.aliases.clear();
            page.removed_ids.clear();
            page.detail = None;
        }
    }
    Ok(page)
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

struct CompleteSeedCommentary {
    pairs: Vec<(String, String)>,
    unpaired_ids: BTreeSet<String>,
}

fn seed_assistant_commentary_candidate(event: &AgentChatEvent) -> bool {
    let Some((_, side)) =
        crate::providers::chat_transcript::codex_display_evidence_from_bounded_header(event)
    else {
        return false;
    };
    match side {
        crate::providers::chat_transcript::CodexDisplaySide::AssistantLive => {
            event.source.as_deref() == Some("event_msg")
                && matches!(
                    event.metadata["raw_type"].as_str(),
                    Some("agent_message" | "assistant_message")
                )
                && event.turn_id.is_none()
                && event.metadata["provider_phase"].as_str() == Some("commentary")
        }
        crate::providers::chat_transcript::CodexDisplaySide::AssistantFinal => {
            event.source.as_deref() == Some("response_item")
                && event.metadata["raw_type"].as_str() == Some("message")
                && event
                    .turn_id
                    .as_deref()
                    .is_some_and(|turn| !turn.is_empty())
                && event.metadata["provider_phase"].as_str() == Some("commentary")
        }
        _ => false,
    }
}

fn complete_seed_commentary_pairs(
    context: &ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    seed: &crate::commands::chat_recent_seed::Seed,
    source_page: Option<&super::chat_source_index::SourcePage>,
) -> Option<CompleteSeedCommentary> {
    let checkpoint = seed.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.normalizer_complete && seed.progress == "provisional" && !seed.events.is_empty()
    })?;
    // A bounded suffix can supply validated observations; only the whole prefix permits local joins.
    let whole_seed = checkpoint.before.is_none() && checkpoint.window_complete;
    let Ok((epoch, admission, extent, _, policy)) =
        crate::commands::chat_recent_seed::source_scope_with_policy(snapshot)
    else {
        return None;
    };
    if epoch != checkpoint.epoch
        || admission != checkpoint.admission
        || extent != checkpoint.extent
        || policy.admission_digest().ok().as_deref() != Some(checkpoint.admission.as_str())
        || source_page.is_some_and(|source_page| source_page.epoch != checkpoint.epoch)
    {
        return None;
    }
    let (admitted_start, admitted_end) = policy.recent_interval(extent)?;
    if policy.recent_interval(admitted_start).is_some()
        || !policy.framed_starts.contains(&admitted_start)
        || !super::chat_source_index::framed_extent(snapshot, &checkpoint.epoch, admitted_end)
            .unwrap_or(false)
    {
        return None;
    }
    let snapshot_path = snapshot.log_path.as_deref()?;
    let expected_path_header = clip(&snapshot_path.to_string_lossy(), 256);
    let path = std::fs::canonicalize(snapshot_path)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())?;
    let proof = super::chat_logical_index::Admission {
        context,
        conversation: None,
        epoch: &checkpoint.epoch,
        admission: &checkpoint.admission,
        path: &path,
        watermark: checkpoint.extent,
        sequence_trusted: false,
    };
    let mut evidence = HashMap::with_capacity(seed.events.len());
    for event in &seed.events {
        let (start, end) = event.metadata["chat_source_start"]
            .as_u64()
            .zip(event.metadata["chat_source_end"].as_u64())?;
        let valid_seed_row = event.metadata["chat_source_ref"].as_str() == Some(event.id.as_str())
            && event.metadata["chat_source_epoch"].as_str() == Some(checkpoint.epoch.as_str())
            && event.metadata["chat_source_admission"].as_str()
                == Some(checkpoint.admission.as_str())
            && start < end
            && end <= checkpoint.extent
            && policy.admits(start, end)
            && event.metadata["log_path"].as_str() == Some(expected_path_header.as_str());
        if !valid_seed_row {
            return None;
        }
        let mut event = event.clone();
        event.metadata["log_path"] = json!(path);
        if !proof.native(&event) || !proof.owned(&event) {
            return None;
        }
        evidence.insert(event.id.clone(), event);
    }
    // A current complete index may join a bounded seed; local inference requires the whole prefix.
    let current_source_page_complete =
        source_page.is_some_and(|page| super::chat_source_index::current_complete(snapshot, page));
    let relations: Vec<(super::chat_logical_index::Relation, Option<String>)> =
        if let Some(source_page) = source_page.filter(|_| current_source_page_complete) {
            source_page
                .events
                .iter()
                .filter_map(|certified| {
                    let members = certified.metadata["chat_display_member_ids"]
                        .as_array()?
                        .iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect::<Vec<_>>();
                    let physical_id = certified.metadata["chat_display_physical_id"].as_str()?;
                    let start = certified.metadata["chat_source_start"].as_u64()?;
                    let end = certified.metadata["chat_source_end"].as_u64()?;
                    if members.len() != 2
                        || !members.iter().any(|member| member == physical_id)
                        || certified.metadata["chat_source_ref"].as_str() != Some(physical_id)
                        || certified.metadata["chat_source_epoch"].as_str()
                            != Some(source_page.epoch.as_str())
                        || certified.metadata["chat_source_admission"].as_str()
                            != Some(source_page.admission.as_str())
                        || start >= end
                        || end > source_page.target_extent
                    {
                        return None;
                    }
                    Some((
                        super::chat_logical_index::Relation {
                            id: certified.id.clone(),
                            members,
                        },
                        Some(physical_id.to_owned()),
                    ))
                })
                .collect()
        } else if whole_seed {
            super::chat_logical_index::Index::bounded_relations(&proof, evidence.values())
                .into_iter()
                .map(|relation| (relation, None))
                .collect()
        } else {
            Vec::new()
        };
    let mut pairs = BTreeSet::new();
    for (relation, physical_id) in relations {
        if relation.members.len() != 2 {
            continue;
        }
        let first_id = relation.members[0].as_str();
        let second_id = relation.members[1].as_str();
        if first_id == second_id {
            continue;
        }
        if let Some(physical_id) = physical_id.as_deref() {
            if ![first_id, second_id].contains(&physical_id)
                || evidence.get(physical_id).is_none_or(|physical| {
                    physical.metadata["chat_source_ref"].as_str() != Some(physical_id)
                })
            {
                continue;
            }
        }
        let (Some(first), Some(second)) = (evidence.get(first_id), evidence.get(second_id)) else {
            continue;
        };
        let Some((first_identity, first_side)) =
            crate::providers::chat_transcript::codex_display_evidence_from_bounded_header(first)
        else {
            continue;
        };
        let Some((second_identity, second_side)) =
            crate::providers::chat_transcript::codex_display_evidence_from_bounded_header(second)
        else {
            continue;
        };
        if first_identity != second_identity {
            continue;
        }
        let (live, completed) = match (first_side, second_side) {
            (
                crate::providers::chat_transcript::CodexDisplaySide::AssistantLive,
                crate::providers::chat_transcript::CodexDisplaySide::AssistantFinal,
            ) => (first, second),
            (
                crate::providers::chat_transcript::CodexDisplaySide::AssistantFinal,
                crate::providers::chat_transcript::CodexDisplaySide::AssistantLive,
            ) => (second, first),
            _ => continue,
        };
        if live.source.as_deref() != Some("event_msg")
            || !matches!(
                live.metadata["raw_type"].as_str(),
                Some("agent_message" | "assistant_message")
            )
            || live.turn_id.is_some()
            || live.metadata["provider_phase"].as_str() != Some("commentary")
            || completed.source.as_deref() != Some("response_item")
            || completed.metadata["raw_type"].as_str() != Some("message")
            || completed.turn_id.as_deref().is_none_or(str::is_empty)
            || completed.metadata["provider_phase"].as_str() != Some("commentary")
            || relation.id != live.id
        {
            continue;
        }
        pairs.insert((live.id.clone(), completed.id.clone()));
    }
    let pairs = pairs.into_iter().collect::<Vec<_>>();
    let paired_ids = pairs
        .iter()
        .flat_map(|(live, completed)| [live.as_str(), completed.as_str()])
        .collect::<BTreeSet<_>>();
    let mut identity_groups = HashMap::<String, (Vec<String>, usize, usize)>::new();
    for event in evidence
        .values()
        .filter(|event| seed_assistant_commentary_candidate(event))
    {
        let Some((identity, side)) =
            crate::providers::chat_transcript::codex_display_evidence_from_bounded_header(event)
        else {
            continue;
        };
        let group = identity_groups.entry(identity).or_default();
        group.0.push(event.id.clone());
        match side {
            crate::providers::chat_transcript::CodexDisplaySide::AssistantLive => group.1 += 1,
            crate::providers::chat_transcript::CodexDisplaySide::AssistantFinal => group.2 += 1,
            _ => {}
        }
    }
    let observed_conflicts = identity_groups
        .into_values()
        .filter(|(_, live_count, final_count)| *live_count > 1 || *final_count > 1)
        .flat_map(|(ids, _, _)| ids)
        .collect::<BTreeSet<_>>();
    // An incomplete bounded suffix cannot prove absence. It can retract a pair only
    // when it contains positive same-side collision evidence.
    let unpaired_ids = if whole_seed || current_source_page_complete {
        evidence
            .values()
            .filter(|event| seed_assistant_commentary_candidate(event))
            .filter(|event| !paired_ids.contains(event.id.as_str()))
            .map(|event| event.id.clone())
            .collect()
    } else {
        observed_conflicts
    };
    Some(CompleteSeedCommentary {
        pairs,
        unpaired_ids,
    })
}

fn apply_seed_commentary_pairs(events: &mut [AgentChatEvent], pairs: &[(String, String)]) {
    for event in events {
        let Some(source_ref) = event.metadata["chat_source_ref"].as_str() else {
            continue;
        };
        let Some((live_id, completed_id)) = pairs.iter().find(|(live_id, completed_id)| {
            source_ref == live_id.as_str() || source_ref == completed_id.as_str()
        }) else {
            continue;
        };
        event.metadata["chat_display_physical_id"] = json!(source_ref);
        event.metadata["chat_display_member_ids"] = json!([live_id, completed_id]);
        event.id.clone_from(live_id);
    }
}

/// Reuse only relations certified by a complete, admission-checked source page.
fn source_page_commentary_pairs(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    source_page: &super::chat_source_index::SourcePage,
) -> Vec<(String, String)> {
    if !super::chat_source_index::current_complete(snapshot, source_page) {
        return Vec::new();
    }
    source_page
        .events
        .iter()
        .filter_map(|event| {
            let members = event.metadata["chat_display_member_ids"].as_array()?;
            if members.len() != 2 {
                return None;
            }
            let members = members
                .iter()
                .map(|member| member.as_str())
                .collect::<Option<Vec<_>>>()?;
            let physical_id = event.metadata["chat_display_physical_id"].as_str()?;
            let source_ref = event.metadata["chat_source_ref"].as_str()?;
            if members[0] == members[1]
                || physical_id != source_ref
                || !members.contains(&physical_id)
                || !members.contains(&event.id.as_str())
                || !seed_assistant_commentary_candidate(event)
            {
                return None;
            }
            let completed_id = members
                .into_iter()
                .find(|member| *member != event.id.as_str())?;
            Some((event.id.clone(), completed_id.to_owned()))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The shared read boundary. Every disk/decode path uses fixed limits, including
/// stale cursors and detail failures. No capture, repair or archive lock occurs.
pub(crate) fn read_with_revalidation(
    context: &ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    cursor: Option<&str>,
    revision: Option<&str>,
    detail_ref: Option<&str>,
    revalidate_ids: Option<&[String]>,
) -> io::Result<AgentChatPage> {
    let Some(revalidate_ids) = revalidate_ids else {
        return read(context, snapshot, cursor, revision, detail_ref);
    };
    let Some(source_ids) = validate_revalidation_request(
        &context.agent_id,
        cursor,
        revision,
        detail_ref,
        Some(revalidate_ids.to_vec()),
    )?
    else {
        return Err(io::Error::other("invalid chat revalidation request"));
    };
    read_revalidated(context, snapshot, revision.unwrap(), &source_ids)
}

pub(crate) fn read(
    context: &ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    cursor: Option<&str>,
    revision: Option<&str>,
    detail_ref: Option<&str>,
) -> io::Result<AgentChatPage> {
    let mut current = head(&context.agent_id).unwrap_or(None);
    let mut store = store(&context.agent_id)?;
    let decoded_head = current
        .as_deref()
        .and_then(|reference| store.read::<Head>(reference).ok());
    // A source key can stay absent or unchanged across provider/session changes.
    // Check cold scope before either the revision shortcut or any source reads.
    let cold_rejected = decoded_head
        .as_ref()
        .is_some_and(|published| !cold_bootstrap::scope_matches(published, context));
    if cold_rejected {
        current = None;
    }
    let source_pointer = super::chat_source_index::pointer(&context.agent_id).unwrap_or(None);
    let previous_seed = revision
        .and_then(|value| value.split_once('.'))
        .filter(|(previous, _)| *previous == current.as_deref().unwrap_or("none"))
        .map(|(_, seed)| seed.split_once('~').map_or(seed, |(seed, _)| seed));
    let previous_source = revision
        .and_then(|value| value.split_once('.'))
        .filter(|(previous, _)| *previous == current.as_deref().unwrap_or("none"))
        .and_then(|(_, seed)| seed.rsplit_once('~').map(|(_, source)| source));
    let source_head_changed = previous_source
        .is_some_and(|previous| previous != source_pointer.as_deref().unwrap_or("none"));
    let mut seed = if cold_rejected {
        crate::commands::chat_recent_seed::Seed {
            revision: "scope_changed".into(),
            progress: "projection_pending".into(),
            ..Default::default()
        }
    } else if cursor.is_none() && detail_ref.is_none() {
        crate::commands::chat_recent_seed::read(snapshot, previous_seed)?
    } else {
        crate::commands::chat_recent_seed::Seed {
            revision: "page".into(),
            progress: "ready".into(),
            ..Default::default()
        }
    };
    let mut token = format!(
        "{}.{}~{}",
        current.as_deref().unwrap_or("none"),
        seed.revision,
        source_pointer.as_deref().unwrap_or("none")
    );
    let mut result = AgentChatPage {
        session_id: context.agent_id.clone(),
        conversation_id: None,
        generation: None,
        source_epoch: None,
        revision: token.clone(),
        events: Vec::new(),
        next_before: None,
        unchanged: false,
        reset: cold_rejected,
        progress: seed.progress.clone(),
        aliases: Vec::new(),
        removed_ids: Vec::new(),
        detail: None,
        revalidated_ids: None,
        bytes_read: seed.bytes_read + 2 * HEAD_BYTES as usize,
        records_decoded: seed.records_decoded,
    };
    if let Some(checkpoint) = &seed.checkpoint {
        result.source_epoch = Some(checkpoint.epoch.clone());
    }
    if current.is_none() && result.progress == "ready" {
        result.progress = "projection_pending".into();
    }
    if cold_rejected {
        result.bytes_read += store.bytes;
        result.records_decoded += store.objects;
        return finish(result);
    }
    let mut current_head = None;
    if current.is_some() {
        match decoded_head {
            Some(published)
                if published.agent_id == context.agent_id
                    && published.source_key == context.provider_source_key =>
            {
                current_head = Some(published)
            }
            _ => {
                result.reset = true;
                result.progress = "projection_pending".into();
            }
        }
    }
    // An unchanged reply still carries the validated scope of the page it
    // acknowledges. Reuse the head already decoded for cold-scope checks.
    if let Some(published) = &current_head {
        result.conversation_id = Some(published.conversation_id.clone());
        result.generation = Some(published.generation.clone());
        result.source_epoch = published.source_epoch.clone();
        if published.progress != "ready"
            && !matches!(
                result.progress.as_str(),
                "oversized_record" | "source_changed" | "ownership_pending"
            )
        {
            result.progress = published.progress.clone();
        }
    }
    let source_scope_attempted = source_pointer.is_some();
    let mut source_scope_valid = !source_scope_attempted;
    if let Some(reference) = &source_pointer {
        result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
        result.records_decoded += 2;
        match super::chat_source_index::scope(snapshot, &mut store, reference) {
            Ok((generation, epoch, _)) => {
                source_scope_valid = true;
                result.generation = Some(generation);
                result.source_epoch = Some(epoch);
            }
            Err(_) => {
                source_scope_valid = false;
                result.generation = None;
                result.reset = true;
                result.progress = "source_changed".into();
            }
        }
    }
    let mut stale_logical_watermark = false;
    let logical_admitted = if matches!(context.provider.as_str(), "codex" | "pi") {
        result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
        result.records_decoded += 2;
        match current_head.as_ref() {
            None => true,
            Some(published) if !published.logical.has_evidence() => true,
            Some(published) => crate::commands::chat_recent_seed::source_scope(snapshot).is_ok_and(
                |(epoch, admission, extent, _)| {
                    let same_scope = published.source_epoch.as_deref() == Some(epoch.as_str())
                        && published.logical.admits(&admission);
                    let watermark = published.logical.completed_watermark();
                    stale_logical_watermark = same_scope
                        && published.progress == "ready"
                        && watermark.is_some_and(|watermark| watermark < extent);
                    same_scope && watermark.is_none_or(|watermark| watermark == extent)
                },
            ),
        }
    } else {
        true
    };
    if !logical_admitted {
        result.reset = true;
    }
    let mut revalidated_seed_commentary = None;
    if revision == Some(token.as_str()) && seed.unchanged {
        if result.reset
            && source_scope_valid
            && stale_logical_watermark
            && !source_head_changed
            && cursor.is_none()
            && detail_ref.is_none()
        {
            let revalidated_seed = crate::commands::chat_recent_seed::read(snapshot, None)?;
            result.bytes_read += revalidated_seed.bytes_read;
            result.records_decoded += revalidated_seed.records_decoded;
            let current_commentary =
                complete_seed_commentary_pairs(context, snapshot, &revalidated_seed, None);
            let same_seed_revision = previous_seed == Some(revalidated_seed.revision.as_str());
            let current_pair_proven = current_commentary
                .as_ref()
                .is_some_and(|projection| !projection.pairs.is_empty());
            if same_seed_revision && current_pair_proven {
                result.progress = revalidated_seed.progress;
                result.source_epoch = revalidated_seed
                    .checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.epoch.clone());
                result.reset = false;
                result.unchanged = true;
                result.bytes_read += store.bytes;
                result.records_decoded += store.objects;
                return finish(result);
            }
            seed = revalidated_seed;
            revalidated_seed_commentary = Some(current_commentary);
            token = format!(
                "{}.{}~{}",
                current.as_deref().unwrap_or("none"),
                seed.revision,
                source_pointer.as_deref().unwrap_or("none")
            );
            result.revision.clone_from(&token);
            result.progress = seed.progress.clone();
            result.source_epoch = seed
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.epoch.clone());
        }
        if revision == Some(token.as_str()) && seed.unchanged && !result.reset {
            result.unchanged = true;
            if current_head.is_none() {
                if let Some(reference) = &source_pointer {
                    result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
                    result.records_decoded += 2;
                    match super::chat_source_index::scope(snapshot, &mut store, reference) {
                        Ok((generation, epoch, progress)) => {
                            result.generation = Some(generation);
                            result.source_epoch = Some(epoch);
                            result.progress = progress;
                        }
                        Err(_) => {
                            result.unchanged = false;
                            result.reset = true;
                            result.progress = "source_changed".into();
                        }
                    }
                }
            }
            result.bytes_read += store.bytes;
            result.records_decoded += store.objects;
            return finish(result);
        }
    }
    if let Some(detail_ref) = detail_ref.filter(|reference| reference.starts_with("seed:")) {
        if let Some(published) = &current_head {
            result.conversation_id = Some(published.conversation_id.clone());
            if !source_scope_attempted {
                result.generation = Some(published.generation.clone());
                result.source_epoch = published.source_epoch.clone();
            }
        } else if let Some(reference) = &source_pointer {
            let (generation, epoch, _) =
                super::chat_source_index::scope(snapshot, &mut store, reference)?;
            result.generation = Some(generation);
            result.source_epoch = Some(epoch);
            result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
            result.records_decoded += 2;
        }
        result.detail = Some(crate::commands::chat_recent_seed::detail(
            snapshot, detail_ref,
        )?);
        result.bytes_read += store.bytes + 2 * (2 * HEAD_BYTES + POLICY_BYTES) as usize + 16 * 1024;
        result.records_decoded += store.objects + 5;
        return finish(result);
    }
    let source_cursor = cursor.is_some_and(|cursor| cursor.starts_with("source:"));
    let mut seed_source_page = None;
    let mut source_page_pairs = Vec::new();
    let mut source_page_reset = false;
    let mut source_page_range_covered = false;
    if (source_head_changed || stale_logical_watermark)
        && current_head.is_some()
        && !source_cursor
        && detail_ref.is_none()
    {
        if let Some(reference) = &source_pointer {
            result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
            result.records_decoded += 2;
            match super::chat_source_index::page(
                snapshot,
                &mut store,
                reference,
                None,
                previous_source.filter(|source| *source != "none"),
            ) {
                Ok(page) => {
                    source_page_pairs = source_page_commentary_pairs(snapshot, &page);
                    source_page_reset = page.reset;
                    result.reset |= page.reset;
                    if !matches!(
                        result.progress.as_str(),
                        "oversized_record" | "source_changed" | "ownership_pending"
                    ) {
                        result.progress = page.progress.clone();
                    }
                    seed_source_page = Some(page);
                }
                Err(_) => {
                    source_page_reset = true;
                    result.reset = true;
                    result.progress = "source_changed".into();
                }
            }
        } else {
            source_page_reset = true;
            result.reset = true;
            result.progress = "source_changed".into();
        }
    }
    if source_cursor || (current_head.is_none() && source_pointer.is_some()) {
        if let Some(reference) = &source_pointer {
            result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
            result.records_decoded += 2;
            match super::chat_source_index::page(
                snapshot,
                &mut store,
                reference,
                cursor.filter(|_| source_cursor),
                previous_source.filter(|source| *source != "none"),
            ) {
                Ok(page) => {
                    source_page_pairs = source_page_commentary_pairs(snapshot, &page);
                    source_page_reset = page.reset;
                    source_page_range_covered = page.range_covered;
                    seed_source_page = Some(page.clone());
                    result.generation = Some(page.generation.clone());
                    result.source_epoch = Some(page.epoch.clone());
                    if !matches!(
                        result.progress.as_str(),
                        "oversized_record" | "source_changed" | "ownership_pending"
                    ) {
                        result.progress = page.progress.clone();
                    }
                    result.reset |= page.reset;
                    if source_cursor {
                        if page.range_covered {
                            result.events = page.events;
                            result.next_before = page.next_before;
                        } else if let Some((before_offset, ordinal)) = page.cursor_frontier {
                            result.events.clear();
                            result.next_before = cursor.map(str::to_owned);
                            if ordinal == 0 {
                                let fallback = crate::commands::chat_recent_seed::read_window(
                                    snapshot,
                                    None,
                                    Some(before_offset),
                                )?;
                                result.bytes_read += fallback.bytes_read;
                                result.records_decoded += fallback.records_decoded;
                                let fallback_scope_matches =
                                    fallback.checkpoint.as_ref().is_some_and(|checkpoint| {
                                        checkpoint.epoch == page.epoch
                                            && checkpoint.admission == page.admission
                                            && checkpoint.extent >= page.target_extent
                                    });
                                if fallback_scope_matches {
                                    let fallback_before =
                                        fallback.events.iter().min_by_key(|event| {
                                            event.metadata["chat_source_start"].as_u64()
                                        });
                                    if !fallback.events.is_empty() {
                                        result.next_before = fallback_before.and_then(|before| {
                                            fallback.checkpoint.as_ref().and_then(|checkpoint| {
                                                super::chat_source_index::continuation(
                                                    snapshot,
                                                    &mut store,
                                                    reference,
                                                    checkpoint,
                                                    Some(before),
                                                )
                                                .ok()
                                                .flatten()
                                            })
                                        });
                                    }
                                    result.events = fallback.events;
                                    let fallback_incomplete =
                                        fallback.checkpoint.as_ref().is_some_and(|checkpoint| {
                                            !checkpoint.window_complete
                                                || !checkpoint.normalizer_complete
                                        });
                                    if matches!(
                                        fallback.progress.as_str(),
                                        "source_changed"
                                            | "ownership_pending"
                                            | "source_unavailable"
                                    ) {
                                        result.progress = fallback.progress;
                                        result.events.clear();
                                        result.next_before = cursor.map(str::to_owned);
                                        if result.progress == "source_changed" {
                                            result.reset = true;
                                        }
                                    } else if fallback.progress == "oversized_record" {
                                        result.progress = fallback.progress;
                                    } else if fallback_incomplete {
                                        result.progress = if page.progress == "ready"
                                            || page.progress == "provisional"
                                        {
                                            "indexing".into()
                                        } else {
                                            page.progress
                                        };
                                    }
                                    if fallback_incomplete {
                                        // Do not let a bounded suffix cursor jump
                                        // past an unresolved physical record.
                                        result.next_before = cursor.map(str::to_owned);
                                    }
                                } else {
                                    let fallback_scope_changed = fallback.progress
                                        == "source_changed"
                                        || fallback.checkpoint.as_ref().is_some_and(|checkpoint| {
                                            checkpoint.epoch != page.epoch
                                                || checkpoint.admission != page.admission
                                                || checkpoint.extent < page.target_extent
                                        });
                                    result.events.clear();
                                    result.next_before = cursor.map(str::to_owned);
                                    result.progress = fallback.progress;
                                    if fallback_scope_changed {
                                        result.reset = true;
                                    }
                                }
                            } else if !matches!(
                                page.progress.as_str(),
                                "source_changed" | "ownership_pending" | "oversized_record"
                            ) {
                                result.progress =
                                    if page.progress == "ready" || page.progress == "provisional" {
                                        "indexing".into()
                                    } else {
                                        page.progress
                                    };
                            }
                        } else {
                            result.events.clear();
                            result.next_before = cursor.map(str::to_owned);
                            result.progress = "source_changed".into();
                            result.reset = true;
                        }
                    } else if page.counts_complete {
                        result.events = page.events;
                        result.next_before = page.next_before;
                    } else {
                        // A sparse forward prefix is neither the recent window
                        // nor a valid source-page frontier.
                        result.events.clear();
                        result.next_before = None;
                    }
                    if let Some(published) = &current_head {
                        result.conversation_id = Some(published.conversation_id.clone());
                        if !source_scope_attempted {
                            result.generation = Some(published.generation.clone());
                            result.source_epoch = published.source_epoch.clone();
                        }
                    }
                }
                Err(_) => {
                    source_page_reset = true;
                    result.reset = true;
                    result.progress = "source_changed".into();
                }
            }
        } else {
            source_page_reset = true;
            result.reset = true;
            result.progress = "projection_pending".into();
        }
    } else if let (Some(reference), Some(published)) = (&current, &current_head) {
        result.conversation_id = Some(published.conversation_id.clone());
        if !source_scope_attempted {
            result.generation = Some(published.generation.clone());
            result.source_epoch = published.source_epoch.clone();
        }
        if let Some(detail_ref) = detail_ref {
            let parts: Vec<_> = detail_ref.split(':').collect();
            if parts.len() != 3 {
                return Err(io::Error::other("invalid chat detail cursor"));
            }
            let pinned: Head = store.read(parts[0])?;
            if pinned.agent_id != context.agent_id || pinned.generation != published.generation {
                return Err(io::Error::other("stale chat detail generation"));
            }
            let offset = parts[2].parse().map_err(io::Error::other)?;
            let original: Row = store.read(parts[1])?;
            if original.agent_id != context.agent_id
                || store.get(&pinned.root, &original.key)?.as_deref() != Some(parts[1])
            {
                return Err(io::Error::other("foreign chat detail"));
            }
            let newest_ref = store
                .get(&published.root, &original.key)?
                .ok_or_else(|| io::Error::other("stale chat body row"))?;
            let newest: Row = store.read(&newest_ref)?;
            if original.event.id != newest.event.id
                || original.body.as_ref().map(|body| &body.binding)
                    != newest.body.as_ref().map(|body| &body.binding)
            {
                return Err(io::Error::other("chat body binding changed"));
            }
            let mut body = detail(&mut store, published, &newest_ref, offset)?;
            body.next = body.next.map(|next| format!("{reference}:{next}"));
            result.detail = Some(body);
        } else if let Some(cursor) = cursor.filter(|_| !source_page_reset) {
            let (pinned_ref, before) = cursor
                .split_once(':')
                .ok_or_else(|| io::Error::other("invalid chat page cursor"))?;
            let pinned: Head = store.read(pinned_ref)?;
            if pinned.agent_id == context.agent_id && pinned.generation == published.generation {
                (result.events, result.next_before) =
                    page(&mut store, reference, published, Some(before))?;
            } else {
                result.reset = true;
                (result.events, result.next_before) = page(&mut store, reference, published, None)?;
            }
        } else {
            let previous = revision
                .and_then(|value| value.split_once('.'))
                .map(|(head, _)| head);
            // Source-index qualification can advance independently of the seed
            // and canonical head. Reset only on its explicit invalidation;
            // otherwise refresh the bounded window when it certifies a pair.
            let refresh_source_projection =
                source_page_reset || (cursor.is_none() && !source_page_pairs.is_empty());
            let delta = if refresh_source_projection {
                None
            } else {
                previous
                    .filter(|previous| valid_ref(previous))
                    .map(|previous| changes_since(&mut store, reference, published, previous))
                    .transpose()?
                    .flatten()
            };
            if let Some(mut delta) = delta {
                delta.sort_by_key(|event| event.sequence);
                result.events = delta;
                let keys = store.page(&published.root, None, PAGE_ROWS + 1)?;
                result.next_before = if keys.len() > PAGE_ROWS || published.progress == "indexing" {
                    keys.get(PAGE_ROWS - 1)
                        .or_else(|| keys.last())
                        .map(|(key, _)| format!("{reference}:{key}"))
                } else {
                    None
                };
            } else {
                result.reset |= previous.is_some();
                (result.events, result.next_before) = page(&mut store, reference, published, None)?;
            }
        }
        if published.progress != "ready"
            && !matches!(
                result.progress.as_str(),
                "oversized_record" | "source_changed" | "ownership_pending"
            )
        {
            result.progress = published.progress.clone();
        }
    } else if cursor.is_some() || detail_ref.is_some() {
        result.reset = true;
    }
    let current_stale_source_page = stale_logical_watermark && !source_page_pairs.is_empty();
    if (logical_admitted || current_stale_source_page)
        && context.provider == "codex"
        && (cursor.is_none() || source_page_reset || current_stale_source_page)
        && detail_ref.is_none()
        && current_head.as_ref().is_some_and(|published| {
            seed_source_page.as_ref().is_some_and(|source_page| {
                published.source_epoch.as_deref() == Some(source_page.epoch.as_str())
            })
        })
    {
        apply_seed_commentary_pairs(&mut result.events, &source_page_pairs);
    }
    let mut unpaired_seed_commentary_ids = BTreeSet::<String>::new();
    let mut preserved_pair_member_ids = BTreeSet::new();
    if cursor.is_none() && detail_ref.is_none() {
        for event in &result.events {
            if let Some(source_ref) = event.metadata["chat_source_ref"].as_str() {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: source_ref.into(),
                        canonical_id: event.id.clone(),
                    });
            }
        }
        let continuation_before = seed
            .events
            .iter()
            .min_by_key(|event| event.metadata["chat_source_start"].as_u64());
        let seed_evidence_usable = seed.checkpoint.as_ref().is_some_and(|checkpoint| {
            checkpoint.normalizer_complete
                && seed.progress == "provisional"
                && !seed.events.is_empty()
        });
        let seed_prefix_complete = seed_evidence_usable
            && seed.checkpoint.as_ref().is_some_and(|checkpoint| {
                checkpoint.before.is_none() && checkpoint.window_complete
            });
        if let (Some(reference), Some(checkpoint), Some(before)) =
            (&source_pointer, &seed.checkpoint, continuation_before)
        {
            if checkpoint.before.is_some()
                && checkpoint.window_complete
                && checkpoint.normalizer_complete
            {
                result.next_before = super::chat_source_index::continuation(
                    snapshot,
                    &mut store,
                    reference,
                    checkpoint,
                    Some(before),
                )
                .ok()
                .flatten();
            }
        } else if result.next_before.is_none() {
            result.next_before = seed_source_page
                .as_ref()
                .filter(|source_page| {
                    super::chat_source_index::current_complete(snapshot, source_page)
                })
                .and_then(|source_page| source_page.next_before.clone());
        }
        let seed_needs_source_page = seed_prefix_complete
            || (seed_evidence_usable
                && seed.events.iter().any(seed_assistant_commentary_candidate));
        if logical_admitted
            && seed_needs_source_page
            && seed_source_page.is_none()
            && store.objects < 380
        {
            if let Some(reference) = &source_pointer {
                result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
                result.records_decoded += 2;
                if let Ok(page) = super::chat_source_index::page(
                    snapshot,
                    &mut store,
                    reference,
                    None,
                    revision.and_then(|r| r.rsplit_once('~').map(|(_, source)| source)),
                ) {
                    result.reset |= page.reset;
                    source_page_pairs = source_page_commentary_pairs(snapshot, &page);
                    seed_source_page = Some(page);
                }
            }
        }
        if seed_prefix_complete {
            if let Some(source_page) = seed_source_page.as_ref().filter(|source_page| {
                super::chat_source_index::current_complete(snapshot, source_page)
            }) {
                result.next_before = source_page.next_before.clone();
            }
        }
        let seed_commentary = revalidated_seed_commentary.take().unwrap_or_else(|| {
            if seed_evidence_usable {
                complete_seed_commentary_pairs(context, snapshot, &seed, seed_source_page.as_ref())
            } else {
                None
            }
        });
        let (current_seed_pairs, seed_unpaired_commentary_ids) = seed_commentary
            .map(|projection| (projection.pairs, projection.unpaired_ids))
            .unwrap_or_default();
        let seed_pairs = current_seed_pairs;
        // Pair creation needs a complete current index or whole seed; observed conflicts still retract.
        unpaired_seed_commentary_ids = seed_unpaired_commentary_ids;
        unpaired_seed_commentary_ids.retain(|id| {
            !source_page_pairs
                .iter()
                .any(|(live_id, completed_id)| id == live_id || id == completed_id)
        });
        let mut recent = seed.events;
        // Resolve newest observations first. Budget exhaustion leaves an
        // explicit unresolved observation, never a text/time guess.
        for event in recent.iter_mut().rev() {
            if unpaired_seed_commentary_ids.contains(&event.id) {
                continue;
            }
            let local = result
                .events
                .iter()
                .find(|row| {
                    row.metadata["chat_source_ref"] == event.metadata["chat_source_ref"]
                        && row.metadata["chat_source_ref"].is_string()
                })
                .cloned();
            let canonical = if local.is_some() {
                local
            } else if store.objects < 380 {
                if let Some(published) = &current_head {
                    alias(&mut store, published, &event.id).ok().flatten()
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(mut canonical) = canonical {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: event.id.clone(),
                        canonical_id: canonical.id.clone(),
                    });
                if let (Some(head_ref), Some(detail_ref)) = (
                    current.as_ref(),
                    canonical.metadata["chat_detail_ref"].as_str(),
                ) {
                    if !detail_ref.starts_with(head_ref) {
                        canonical.metadata["chat_detail_ref"] =
                            json!(format!("{head_ref}:{detail_ref}"));
                    }
                }
                *event = canonical;
            }
        }
        apply_seed_commentary_pairs(&mut result.events, &seed_pairs);
        apply_seed_commentary_pairs(&mut recent, &seed_pairs);
        let stale_display_ids = result
            .events
            .iter()
            .filter_map(|event| {
                let intersects_unpaired = event.metadata["chat_display_member_ids"]
                    .as_array()
                    .is_some_and(|members| {
                        members
                            .iter()
                            .filter_map(|member| member.as_str())
                            .any(|member| unpaired_seed_commentary_ids.contains(member))
                    });
                intersects_unpaired.then(|| event.id.clone())
            })
            .collect::<BTreeSet<_>>();
        result
            .events
            .retain(|event| !stale_display_ids.contains(&event.id));
        for stale_id in stale_display_ids {
            if !unpaired_seed_commentary_ids.contains(&stale_id)
                && !result.removed_ids.contains(&stale_id)
            {
                result.removed_ids.push(stale_id);
            }
        }
        for alias in &mut result.aliases {
            if unpaired_seed_commentary_ids.contains(&alias.observation_id) {
                alias.canonical_id.clone_from(&alias.observation_id);
                continue;
            }
            if let Some((live_id, _)) = seed_pairs.iter().find(|(live_id, completed_id)| {
                alias.observation_id == live_id.as_str()
                    || alias.observation_id == completed_id.as_str()
            }) {
                alias.canonical_id.clone_from(live_id);
            }
        }
        for observation_id in &unpaired_seed_commentary_ids {
            if !result
                .aliases
                .iter()
                .any(|alias| alias.observation_id == *observation_id)
            {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: observation_id.clone(),
                        canonical_id: observation_id.clone(),
                    });
            }
        }
        if !logical_admitted && stale_logical_watermark {
            for event in &result.events {
                let Some(members) = event.metadata["chat_display_member_ids"]
                    .as_array()
                    .filter(|members| members.len() == 2)
                    .and_then(|members| {
                        let members = members
                            .iter()
                            .map(|member| member.as_str())
                            .collect::<Option<Vec<_>>>()?;
                        let physical_id = event.metadata["chat_display_physical_id"].as_str()?;
                        let valid_pair = members[0] != members[1]
                            && members.contains(&event.id.as_str())
                            && members.contains(&physical_id)
                            && event.metadata["chat_source_ref"].as_str() == Some(physical_id)
                            && members
                                .iter()
                                .all(|member| !unpaired_seed_commentary_ids.contains(*member));
                        valid_pair
                            .then(|| members.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    })
                else {
                    continue;
                };
                preserved_pair_member_ids.extend(members);
            }
        }
        for event in recent {
            if preserved_pair_member_ids.contains(&event.id) {
                continue;
            }
            if let Some(index) = result.events.iter().position(|old| old.id == event.id) {
                result.events[index] = event;
            } else {
                result.events.push(event);
            }
        }
        if result.events.len() > PAGE_ROWS {
            result.events.drain(..result.events.len() - PAGE_ROWS);
        }
    }
    let mut current_canonical_retractions = BTreeSet::<String>::new();
    let mut current_canonical_replacements = BTreeSet::<String>::new();
    let mut defer_source_page_for_unchecked_relation = false;
    if source_scope_valid
        && (logical_admitted || stale_logical_watermark)
        && source_cursor
        && source_page_range_covered
    {
        if let Some(published) = &current_head {
            let mut saved_relation_refs = BTreeSet::<String>::new();
            for event in &result.events {
                let Some(coordinate) = event.metadata["chat_source_ref"].as_str() else {
                    continue;
                };
                if unpaired_seed_commentary_ids.contains(coordinate) {
                    continue;
                }
                let Some(members) = event.metadata["chat_display_member_ids"]
                    .as_array()
                    .filter(|members| members.len() == 2)
                    .and_then(|members| {
                        members
                            .iter()
                            .map(|member| member.as_str())
                            .collect::<Option<Vec<_>>>()
                    })
                else {
                    continue;
                };
                let Some(physical_id) = event.metadata["chat_display_physical_id"].as_str() else {
                    continue;
                };
                if members[0] != members[1]
                    && members.contains(&event.id.as_str())
                    && members.contains(&physical_id)
                    && coordinate == physical_id
                {
                    saved_relation_refs.insert(coordinate.to_owned());
                }
            }
            let native_page_complete = seed_source_page
                .as_ref()
                .is_some_and(|source_page| source_page.counts_complete);
            let mut canonical_by_source_ref = HashMap::new();
            for coordinate in &saved_relation_refs {
                if store.objects >= 380 {
                    defer_source_page_for_unchecked_relation = true;
                    break;
                }
                match alias(&mut store, published, coordinate) {
                    Ok(canonical) => {
                        canonical_by_source_ref.insert(coordinate.clone(), canonical);
                    }
                    Err(_) => {
                        defer_source_page_for_unchecked_relation = true;
                        break;
                    }
                }
            }
            if !defer_source_page_for_unchecked_relation {
                for event in result.events.iter_mut().rev() {
                    let Some(coordinate) = event.metadata["chat_source_ref"].as_str() else {
                        continue;
                    };
                    if unpaired_seed_commentary_ids.contains(coordinate) {
                        continue;
                    }
                    let saved_relation = saved_relation_refs.contains(coordinate);
                    if !native_page_complete && !saved_relation {
                        continue;
                    }
                    let canonical = match canonical_by_source_ref.get(coordinate) {
                        Some(canonical) => canonical.clone(),
                        None if store.objects < 380 => {
                            alias(&mut store, published, coordinate).ok().flatten()
                        }
                        None => continue,
                    };
                    if let Some(mut canonical) = canonical {
                        let current_members = canonical.metadata["chat_display_member_ids"]
                            .as_array()
                            .filter(|members| members.len() == 2)
                            .and_then(|members| {
                                let members = members
                                    .iter()
                                    .map(|member| member.as_str())
                                    .collect::<Option<Vec<_>>>()?;
                                let physical_id =
                                    canonical.metadata["chat_display_physical_id"].as_str()?;
                                let valid_pair = members[0] != members[1]
                                    && members.contains(&canonical.id.as_str())
                                    && members.contains(&physical_id)
                                    && members.contains(&coordinate);
                                valid_pair.then_some(members)
                            });
                        if let Some(current_members) = current_members {
                            current_canonical_replacements
                                .extend(current_members.iter().map(|member| (*member).to_owned()));
                            if let (Some(head_ref), Some(detail_ref)) = (
                                current.as_deref(),
                                canonical.metadata["chat_detail_ref"].as_str(),
                            ) {
                                let detail_prefix = format!("{head_ref}:");
                                if !detail_ref.starts_with(&detail_prefix) {
                                    canonical.metadata["chat_detail_ref"] =
                                        json!(format!("{detail_prefix}{detail_ref}"));
                                }
                            }
                            *event = canonical;
                            continue;
                        }

                        let prior_members = event.metadata["chat_display_member_ids"]
                            .as_array()
                            .map(|members| {
                                members
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let prior_public_id = event.id.clone();
                        let removed_public_id = canonical.metadata["chat_display_removed_ids"]
                            .as_array()
                            .and_then(|removed| {
                                removed
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .find(|id| {
                                        *id == prior_public_id.as_str()
                                            || prior_members
                                                .iter()
                                                .any(|member| member.as_str() == *id)
                                    })
                            });
                        if let Some(removed_public_id) = removed_public_id {
                            let source_page = seed_source_page.as_ref();
                            let source_ref = event.metadata["chat_source_ref"].as_str();
                            let source_physical_id = event.metadata["chat_display_physical_id"]
                                .as_str()
                                .unwrap_or(event.id.as_str());
                            let source_start = event.metadata["chat_source_start"].as_u64();
                            let source_end = event.metadata["chat_source_end"].as_u64();
                            let canonical_physical_id = canonical.metadata
                                ["chat_display_physical_id"]
                                .as_str()
                                .unwrap_or(canonical.id.as_str());
                            let same_source = source_scope_valid
                                && source_page_range_covered
                                && source_page.is_some_and(|source_page| {
                                    source_page.range_covered
                                        && source_ref == Some(coordinate)
                                        && source_ref == Some(source_physical_id)
                                        && canonical.metadata["chat_source_ref"].as_str()
                                            == source_ref
                                        && canonical_physical_id == source_physical_id
                                        && source_start.is_some()
                                        && source_end.is_some()
                                        && canonical.metadata["chat_source_start"].as_u64()
                                            == source_start
                                        && canonical.metadata["chat_source_end"].as_u64()
                                            == source_end
                                        && event.metadata["chat_source_epoch"].as_str()
                                            == Some(source_page.epoch.as_str())
                                        && canonical.metadata["chat_source_epoch"].as_str()
                                            == Some(source_page.epoch.as_str())
                                        && event.metadata["chat_source_admission"]
                                            .as_str()
                                            .is_none_or(|admission| {
                                                admission == source_page.admission.as_str()
                                            })
                                        && canonical.metadata["chat_source_admission"]
                                            .as_str()
                                            .is_none_or(|admission| {
                                                admission == source_page.admission.as_str()
                                            })
                                });
                            let body_binding = canonical.metadata["chat_body_binding"].as_str();
                            let detail_ref = canonical.metadata["chat_detail_ref"].as_str();
                            if same_source
                                && body_binding.is_some()
                                && detail_ref.is_some()
                                && canonical.metadata["chat_body_pending"] != true
                                && canonical.metadata["chat_body_unavailable"] != true
                            {
                                if let (Some(head_ref), Some(body_binding), Some(detail_ref)) =
                                    (current.as_deref(), body_binding, detail_ref)
                                {
                                    let detail_prefix = format!("{head_ref}:");
                                    let detail_suffix = detail_ref
                                        .strip_prefix(&detail_prefix)
                                        .unwrap_or(detail_ref);
                                    event.metadata["chat_body_binding"] = json!(body_binding);
                                    event.metadata["chat_detail_ref"] =
                                        json!(format!("{detail_prefix}{detail_suffix}"));
                                    event.metadata["chat_body_pending"] =
                                        canonical.metadata["chat_body_pending"].clone();
                                }
                            }
                            current_canonical_retractions.insert(prior_public_id);
                            current_canonical_retractions.extend(prior_members);
                            if !result
                                .removed_ids
                                .iter()
                                .any(|removed| removed == removed_public_id)
                            {
                                result.removed_ids.push(removed_public_id.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    // A current canonical row with a valid replacement pair wins over its
    // historical removal marker. An explicit removal with no replacement
    // retracts the saved projection even while the broader logical index is
    // still admitted.
    current_canonical_retractions.retain(|id| !current_canonical_replacements.contains(id));
    result
        .removed_ids
        .retain(|id| !current_canonical_replacements.contains(id));
    for event in &mut result.events {
        let Some(members) = event.metadata["chat_display_member_ids"]
            .as_array()
            .filter(|members| members.len() == 2)
            .and_then(|members| {
                members
                    .iter()
                    .map(|member| member.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
        else {
            continue;
        };
        if !members
            .iter()
            .any(|member| current_canonical_retractions.contains(member))
        {
            continue;
        }
        let Some(physical_id) = event.metadata["chat_display_physical_id"]
            .as_str()
            .filter(|physical_id| members.iter().any(|member| member == physical_id))
            .map(str::to_owned)
        else {
            continue;
        };
        if event.metadata["chat_source_ref"].as_str() != Some(physical_id.as_str()) {
            continue;
        }
        event.id.clone_from(&physical_id);
        if let Some(metadata) = event.metadata.as_object_mut() {
            metadata.remove("chat_display_member_ids");
            metadata.remove("chat_display_physical_id");
            metadata.remove("chat_display_removed_ids");
        }
    }
    result
        .aliases
        .retain(|alias| !current_canonical_retractions.contains(&alias.observation_id));
    for observation_id in &current_canonical_retractions {
        if result.events.iter().any(|event| {
            event.id == *observation_id
                && event.metadata["chat_source_ref"].as_str() == Some(observation_id.as_str())
        }) && !result
            .aliases
            .iter()
            .any(|alias| alias.observation_id == *observation_id)
        {
            result
                .aliases
                .push(wardian_core::models::chat::AgentChatAlias {
                    observation_id: observation_id.clone(),
                    canonical_id: observation_id.clone(),
                });
        }
    }
    if !logical_admitted {
        result.aliases.clear();
        for event in &mut result.events {
            let source_ref = event.metadata["chat_source_ref"]
                .as_str()
                .map(str::to_owned);
            let current_pair = if stale_logical_watermark {
                source_ref.as_deref().and_then(|source_ref| {
                    source_page_pairs.iter().find(|(live_id, completed_id)| {
                        source_ref == live_id.as_str() || source_ref == completed_id.as_str()
                    })
                })
            } else {
                None
            };
            if let (Some(source_ref), Some((live_id, completed_id))) =
                (source_ref.as_deref(), current_pair)
            {
                event.id.clone_from(live_id);
                event.metadata["chat_display_physical_id"] = json!(source_ref);
                event.metadata["chat_display_member_ids"] = json!([live_id, completed_id]);
                if let Some(metadata) = event.metadata.as_object_mut() {
                    metadata.remove("chat_display_removed_ids");
                }
                for observation_id in [live_id, completed_id] {
                    result
                        .aliases
                        .push(wardian_core::models::chat::AgentChatAlias {
                            observation_id: observation_id.clone(),
                            canonical_id: live_id.clone(),
                        });
                }
                continue;
            }
            let retained_pair_members = if stale_logical_watermark {
                event.metadata["chat_display_member_ids"]
                    .as_array()
                    .filter(|members| members.len() == 2)
                    .and_then(|members| {
                        let members = members
                            .iter()
                            .map(|member| member.as_str())
                            .collect::<Option<Vec<_>>>()?;
                        let physical_id = event.metadata["chat_display_physical_id"].as_str()?;
                        let valid_pair = members[0] != members[1]
                            && members.contains(&event.id.as_str())
                            && members.contains(&physical_id)
                            && event.metadata["chat_source_ref"].as_str() == Some(physical_id)
                            && members.iter().all(|member| {
                                !unpaired_seed_commentary_ids.contains(*member)
                                    && !current_canonical_retractions.contains(*member)
                            });
                        valid_pair
                            .then(|| members.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    })
            } else {
                None
            };
            if let Some(members) = retained_pair_members {
                for observation_id in members {
                    preserved_pair_member_ids.insert(observation_id.clone());
                    result
                        .aliases
                        .push(wardian_core::models::chat::AgentChatAlias {
                            observation_id,
                            canonical_id: event.id.clone(),
                        });
                }
                continue;
            }
            if let Some(physical) = event.metadata["chat_display_physical_id"]
                .as_str()
                .map(str::to_owned)
            {
                event.id = physical;
                if let Some(metadata) = event.metadata.as_object_mut() {
                    metadata.remove("chat_display_member_ids");
                    metadata.remove("chat_display_removed_ids");
                }
            }
        }
        for observation_id in unpaired_seed_commentary_ids
            .iter()
            .chain(current_canonical_retractions.iter())
        {
            if result.events.iter().any(|event| {
                event.id == *observation_id
                    && event.metadata["chat_source_ref"].as_str() == Some(observation_id.as_str())
            }) && !result
                .aliases
                .iter()
                .any(|alias| alias.observation_id == *observation_id)
            {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: observation_id.clone(),
                        canonical_id: observation_id.clone(),
                    });
            }
        }
    }
    if defer_source_page_for_unchecked_relation {
        result.events.clear();
        result.aliases.clear();
        result.removed_ids.clear();
        result.next_before = cursor.map(str::to_owned);
        if !matches!(
            result.progress.as_str(),
            "oversized_record" | "source_changed" | "ownership_pending"
        ) {
            result.progress = "indexing".into();
        }
        result.unchanged = false;
    }
    project_display_result(&mut result);
    result.bytes_read += store.bytes;
    result.records_decoded += store.objects;
    finish(result)
}

fn revision_matches_heads(revision: &str, canonical: Option<&str>, source: Option<&str>) -> bool {
    let Some((pinned_canonical, body)) = revision.split_once('.') else {
        return false;
    };
    let Some((_, pinned_source)) = body.rsplit_once('~') else {
        return false;
    };
    pinned_canonical == canonical.unwrap_or("none") && pinned_source == source.unwrap_or("none")
}

fn read_revalidated(
    context: &ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    revision: &str,
    source_ids: &[String],
) -> io::Result<AgentChatPage> {
    let mut store = store(&context.agent_id)?;
    let canonical_ref = head(&context.agent_id)?;
    let mut published = canonical_ref
        .as_deref()
        .and_then(|reference| store.read::<Head>(reference).ok());
    let canonical_rejected = published.as_ref().is_some_and(|value| {
        value.agent_id != context.agent_id
            || value.source_key != context.provider_source_key
            || !cold_bootstrap::scope_matches(value, context)
    });
    if canonical_rejected {
        published = None;
    }
    let effective_canonical = (!canonical_rejected)
        .then_some(canonical_ref.as_deref())
        .flatten();
    let canonical_for_bodies = published.clone();
    let source_pointer = super::chat_source_index::pointer(&context.agent_id)?;
    let pointers_match =
        revision_matches_heads(revision, effective_canonical, source_pointer.as_deref());

    let mut generation = None;
    let mut epoch = None;
    let mut progress = "indexing".to_string();
    let mut events = Vec::new();
    let mut complete = false;
    let mut body_required = Vec::new();
    let mut source_admission = None;
    if let Some(reference) = source_pointer.as_deref() {
        if pointers_match && !canonical_rejected {
            let rows = super::chat_source_index::revalidate_rows(
                snapshot, &mut store, reference, source_ids,
            )?;
            generation = (!rows.generation.is_empty()).then_some(rows.generation);
            epoch = (!rows.epoch.is_empty()).then_some(rows.epoch);
            progress = rows.progress;
            complete = rows.complete;
            events = rows.events;
            body_required = rows.body_required;
            source_admission = Some(rows.admission);
        } else if let Ok((current_generation, current_epoch, _)) =
            super::chat_source_index::scope(snapshot, &mut store, reference)
        {
            generation = Some(current_generation);
            epoch = Some(current_epoch);
            progress = "source_changed".into();
        } else {
            progress = "source_changed".into();
        }
    }

    if complete && !body_required.is_empty() {
        let Some(canonical) = canonical_for_bodies.as_ref() else {
            complete = false;
            progress = "body_pending".into();
            body_required.clear();
            return finish_revalidated(
                context,
                RevalidatedPageFields {
                    revision,
                    published,
                    generation,
                    epoch,
                    progress,
                    events,
                    complete,
                    source_ids,
                },
                &mut store,
            );
        };
        for (source_id, source_ref) in body_required {
            let Some(source_row) = events.iter_mut().find(|event| {
                event.metadata["chat_source_ref"].as_str() == Some(source_ref.as_str())
                    && event
                        .metadata
                        .get("chat_display_physical_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(event.id.as_str())
                        == source_id
            }) else {
                complete = false;
                progress = "body_pending".into();
                break;
            };
            let source_start = source_row.metadata["chat_source_start"].as_u64();
            let source_end = source_row.metadata["chat_source_end"].as_u64();
            let canonical_row = match alias(&mut store, canonical, &source_ref) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    complete = false;
                    progress = "body_pending".into();
                    break;
                }
                Err(error) if error.to_string().contains("budget") => {
                    complete = false;
                    progress = "body_pending".into();
                    break;
                }
                Err(error) => return Err(error),
            };
            let canonical_physical_id = canonical_row.metadata["chat_display_physical_id"]
                .as_str()
                .unwrap_or(canonical_row.id.as_str());
            let same_source = canonical_row.metadata["chat_source_ref"].as_str()
                == Some(source_ref.as_str())
                && canonical_physical_id == source_id
                && canonical_row.metadata["chat_source_start"].as_u64() == source_start
                && canonical_row.metadata["chat_source_end"].as_u64() == source_end
                && canonical_row.metadata["chat_source_epoch"].as_str() == epoch.as_deref()
                && canonical_row.metadata["chat_source_admission"]
                    .as_str()
                    .is_none_or(|admission| Some(admission) == source_admission.as_deref());
            let body_binding = canonical_row.metadata["chat_body_binding"].as_str();
            let detail_ref = canonical_row.metadata["chat_detail_ref"].as_str();
            if !same_source
                || body_binding.is_none()
                || detail_ref.is_none()
                || canonical_row.metadata["chat_body_pending"] == true
                || canonical_row.metadata["chat_body_unavailable"] == true
            {
                complete = false;
                progress = "body_pending".into();
                break;
            }
            let body_binding = body_binding.expect("body binding was validated above");
            let detail_ref = detail_ref.expect("detail reference was validated above");
            source_row.metadata["chat_body_binding"] = serde_json::json!(body_binding);
            let Some(canonical_reference) = canonical_ref.as_deref() else {
                complete = false;
                progress = "body_pending".into();
                break;
            };
            source_row.metadata["chat_detail_ref"] =
                serde_json::json!(format!("{canonical_reference}:{detail_ref}"));
            source_row.metadata["chat_body_pending"] =
                canonical_row.metadata["chat_body_pending"].clone();
        }
    }

    let mut result = AgentChatPage {
        session_id: context.agent_id.clone(),
        conversation_id: published
            .as_ref()
            .map(|value| value.conversation_id.clone()),
        generation,
        source_epoch: epoch,
        revision: revision.to_owned(),
        events,
        next_before: None,
        unchanged: false,
        reset: canonical_rejected || progress == "source_changed",
        progress,
        aliases: Vec::new(),
        removed_ids: Vec::new(),
        detail: None,
        revalidated_ids: complete.then(|| source_ids.to_vec()),
        bytes_read: store.bytes,
        records_decoded: store.objects,
    };
    for event in &result.events {
        if event.metadata["chat_display_member_ids"]
            .as_array()
            .is_none()
        {
            if let Some(source_ref) = event.metadata["chat_source_ref"].as_str() {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: source_ref.to_owned(),
                        canonical_id: event.id.clone(),
                    });
            }
        }
    }
    project_display_result(&mut result);
    finish(result)
}

struct RevalidatedPageFields<'a> {
    revision: &'a str,
    published: Option<Head>,
    generation: Option<String>,
    epoch: Option<String>,
    progress: String,
    events: Vec<AgentChatEvent>,
    complete: bool,
    source_ids: &'a [String],
}

// Assemble the reply from separately validated scope, completeness, and body results.
fn finish_revalidated(
    context: &ConversationArchiveContext,
    fields: RevalidatedPageFields<'_>,
    store: &mut Store,
) -> io::Result<AgentChatPage> {
    let RevalidatedPageFields {
        revision,
        published,
        generation,
        epoch,
        progress,
        events,
        complete,
        source_ids,
    } = fields;
    let mut result = AgentChatPage {
        session_id: context.agent_id.clone(),
        conversation_id: published.map(|value| value.conversation_id),
        generation,
        source_epoch: epoch,
        revision: revision.to_owned(),
        events,
        next_before: None,
        unchanged: false,
        reset: false,
        progress,
        aliases: Vec::new(),
        removed_ids: Vec::new(),
        detail: None,
        revalidated_ids: complete.then(|| source_ids.to_vec()),
        bytes_read: store.bytes,
        records_decoded: store.objects,
    };
    for event in &result.events {
        if event.metadata["chat_display_member_ids"]
            .as_array()
            .is_none()
        {
            if let Some(source_ref) = event.metadata["chat_source_ref"].as_str() {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: source_ref.to_owned(),
                        canonical_id: event.id.clone(),
                    });
            }
        }
    }
    project_display_result(&mut result);
    finish(result)
}

pub(crate) fn revalidation_page_is_current(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    page: &AgentChatPage,
) -> bool {
    if page.revalidated_ids.is_none() {
        return true;
    }
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    let Some(reference) = page
        .revision
        .rsplit_once('~')
        .map(|(_, source)| source)
        .filter(|source| valid_ref(source))
    else {
        return false;
    };
    let Some((canonical_reference, _)) = page.revision.split_once('.') else {
        return false;
    };
    let Ok(current_canonical) = head(&context.agent_id) else {
        return false;
    };
    if canonical_reference != current_canonical.as_deref().unwrap_or("none") {
        return false;
    }
    if super::chat_source_index::pointer(&context.agent_id)
        .ok()
        .flatten()
        .as_deref()
        != Some(reference)
    {
        return false;
    }
    let (Some(generation), Some(epoch)) =
        (page.generation.as_deref(), page.source_epoch.as_deref())
    else {
        return false;
    };
    super::chat_source_index::current_complete_scope(snapshot, reference, generation, epoch)
        && !page.reset
}

fn project_display_result(result: &mut AgentChatPage) {
    let mut positions: HashMap<String, usize> = HashMap::new();
    let mut events: Vec<AgentChatEvent> = Vec::new();
    for event in std::mem::take(&mut result.events) {
        if let Some(members) = event.metadata["chat_display_member_ids"].as_array() {
            for member in members.iter().filter_map(|m| m.as_str()).take(4) {
                result
                    .aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id: member.into(),
                        canonical_id: event.id.clone(),
                    });
            }
        }
        if let Some(removed) = event.metadata["chat_display_removed_ids"].as_array() {
            for id in removed.iter().filter_map(|m| m.as_str()).take(4) {
                if !result.removed_ids.iter().any(|known| known == id) {
                    result.removed_ids.push(id.into());
                }
            }
        }
        if let Some(&index) = positions.get(&event.id) {
            if events[index].sequence <= event.sequence {
                events[index] = event;
            }
        } else {
            positions.insert(event.id.clone(), events.len());
            events.push(event);
        }
    }
    // A relation re-established at this watermark supersedes an earlier removal.
    result.removed_ids.retain(|id| !positions.contains_key(id));
    result.aliases.sort_by(|a, b| {
        (&a.observation_id, &a.canonical_id).cmp(&(&b.observation_id, &b.canonical_id))
    });
    result
        .aliases
        .dedup_by(|a, b| a.observation_id == b.observation_id && a.canonical_id == b.canonical_id);
    result.events = events;
}

#[cfg(test)]
#[path = "chat_read_tests.rs"]
mod tests;
