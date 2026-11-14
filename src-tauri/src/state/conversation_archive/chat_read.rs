//! The capture owner builds immutable display indexes in bounded checkpoints.
//! Normal Chat never enters the archive gate or reads raw archive JSONL.
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use serde_json::json;
use wardian_core::conversations::write_json_atomic;
use wardian_core::models::chat::{AgentChatEvent, AgentChatPage};
use wardian_core::paths::agent_conversations_dir;

use super::chat_read_store::{digest, valid_ref, Dependency, SeekCache, Store};
use super::ConversationArchiveContext;

#[path = "chat_archive_bootstrap.rs"]
mod cold_bootstrap;
#[path = "chat_read_packed_recent.rs"]
pub(super) mod packed_recent;

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

impl CanonicalSourceCoverage {
    fn matches(&self, current: &super::chat_source_index::Qualification) -> bool {
        current.complete
            && self.normalizer_settled
            && current.source_key.as_deref() == Some(self.source_key.as_str())
            && self.epoch == current.epoch
            && self.admission == current.admission
            && self.extent == current.extent
    }
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
                .chain(self.recent_page.iter())
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
    recent: Arc<Mutex<RecentSlots>>,
    #[cfg(test)]
    fail_checkpoint_stage: std::sync::atomic::AtomicU8,
}

struct RecentSlot {
    agent_id: String,
    runtime: Weak<Mutex<String>>,
    watch: Weak<Mutex<crate::state::AgentWatchState>>,
    scope: String,
    request_scope: String,
    watch_cursor: Option<String>,
    admission: String,
    issued: u64,
    published: bool,
    version: String,
    headers: Vec<AgentChatEvent>,
    coverage: Vec<RecentCoverage>,
    native_source: Option<crate::commands::chat_recent_database::Source>,
    charge: usize,
}

#[derive(Clone, Serialize)]
struct RecentCoverage {
    event_id: String,
    key: String,
    version: String,
    native_key: Option<String>,
}

struct RecentSlots {
    revision_owner: String,
    slots: VecDeque<RecentSlot>,
    boundaries: BTreeMap<String, u32>,
    global_boundaries: u64,
    pending: Vec<PendingRecent>,
}

struct PendingRecent {
    agent_id: String,
    runtime: Weak<Mutex<String>>,
    watch: Weak<Mutex<crate::state::AgentWatchState>>,
    nonce: String,
}

/// Keeps an actual privacy mutation fenced across its IO and error paths.
/// It owns no work lock, source handle or provider payload.
pub(crate) struct RecentBoundary {
    recent: Arc<Mutex<RecentSlots>>,
    agent_id: Option<String>,
}

impl Drop for RecentBoundary {
    fn drop(&mut self) {
        let Ok(mut recent) = self.recent.lock() else {
            return;
        };
        if let Some(agent_id) = &self.agent_id {
            recent.slots.retain(|slot| &slot.agent_id != agent_id);
            if let Some(count) = recent.boundaries.get_mut(agent_id) {
                *count -= 1;
                if *count == 0 {
                    recent.boundaries.remove(agent_id);
                }
            }
        } else {
            recent.slots.clear();
            recent.global_boundaries -= 1;
        }
    }
}

impl Default for RecentSlots {
    fn default() -> Self {
        Self {
            revision_owner: uuid::Uuid::new_v4().to_string(),
            slots: VecDeque::new(),
            boundaries: BTreeMap::new(),
            global_boundaries: 0,
            pending: Vec::new(),
        }
    }
}

fn recent_scope(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
) -> io::Result<(String, usize)> {
    let (policy, policy_bytes) = super::storage::read_recent_capture_state(&snapshot.session_id)?;
    let (global_logging, logging_bytes) =
        crate::utils::shell::recent_conversation_logging().map_err(io::Error::other)?;
    let live = snapshot
        .watch_state
        .lock()
        .map_err(|_| io::Error::other("recent watch poisoned"))?
        .live_chat_boundary()
        .cloned()
        .filter(|_| {
            super::effective_conversation_logging(
                global_logging,
                snapshot.agent_conversation_logging,
            ) == wardian_core::conversations::ConversationLoggingSetting::Disabled
        });
    let native = snapshot
        .log_path
        .as_deref()
        .map(|path| {
            if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
                return Err(io::Error::other("linked recent source"));
            }
            crate::commands::provider_log_acquisition::native_file_identity(&File::open(path)?)
        })
        .transpose()?;
    Ok((digest(&serde_json::to_vec(&json!({
        "provider": snapshot.provider, "resume": snapshot.resume_session,
        "fresh": snapshot.fresh_provider_session_id, "cleared": snapshot.cleared_provider_sessions,
        "path": snapshot.log_path, "native": native, "logging": snapshot.agent_conversation_logging,
        // Disabled durable capture records every observed ID/cutoff. A proven
        // fresh Watch has a separate LIVE purpose and no durable capability.
        "policy": if let Some(boundary) = live { json!({ "live_watch": boundary }) } else { json!(policy.recent_policy_stamp()?) },
        "global_logging": global_logging,
    })).map_err(io::Error::other)?), policy_bytes + logging_bytes))
}

fn recent_request_scope(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
) -> io::Result<String> {
    Ok(digest(&serde_json::to_vec(&json!({
        "provider": snapshot.provider, "resume": snapshot.resume_session,
        "fresh": snapshot.fresh_provider_session_id, "cleared": snapshot.cleared_provider_sessions,
        "path": snapshot.log_path, "logging": snapshot.agent_conversation_logging,
    })).map_err(io::Error::other)?))
}

impl ProjectionOwner {
    pub(crate) fn recent_boundary(&self, agent_id: Option<&str>) -> io::Result<RecentBoundary> {
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        // Reserve one control entry for a global fence. At extreme concurrent
        // boundary pressure, withholding every preview is the safe fallback.
        let agent_id = agent_id.filter(|agent| {
            agent.len() <= 256
                && (recent.boundaries.contains_key(*agent) || recent.boundaries.len() < 509)
        });
        if let Some(agent) = agent_id {
            recent.slots.retain(|slot| slot.agent_id != agent);
            let count = recent.boundaries.entry(agent.to_owned()).or_default();
            *count = count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("recent boundary count exhausted"))?;
        } else {
            recent.slots.clear();
            recent.global_boundaries = recent
                .global_boundaries
                .checked_add(1)
                .ok_or_else(|| io::Error::other("recent global boundary count exhausted"))?;
        }
        trim_recent_slots(&mut recent);
        Ok(RecentBoundary {
            recent: self.recent.clone(),
            agent_id: agent_id.map(str::to_owned),
        })
    }

    /// Admit before old archive work. The token fences IO finishing after a
    /// runtime, Watch, source or privacy retirement, including A-B-A changes.
    pub(crate) fn begin_recent_with_source(
        &self,
        snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
        runtime: &Arc<Mutex<String>>,
        native_source: Option<crate::commands::chat_recent_database::Source>,
    ) -> io::Result<(String, u64)> {
        if native_source
            .as_ref()
            .is_some_and(|source| !source.valid().unwrap_or(false))
        {
            return Err(io::Error::other("recent native source retired"));
        }
        if snapshot.session_id.len() > 256 {
            return Err(io::Error::other("recent agent identity exceeds byte limit"));
        }
        let (scope, _) = recent_scope(snapshot)?;
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if recent.global_boundaries > 0 || recent.boundaries.contains_key(&snapshot.session_id) {
            return Err(io::Error::other("recent privacy boundary pending"));
        }
        if let Some(slot) = recent
            .slots
            .iter_mut()
            .find(|slot| slot.agent_id == snapshot.session_id)
        {
            if slot.scope == scope
                && slot.native_source == native_source
                && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
            {
                slot.issued = slot
                    .issued
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("recent issue exhausted"))?;
                return Ok((slot.admission.clone(), slot.issued));
            }
        }
        recent
            .slots
            .retain(|slot| slot.agent_id != snapshot.session_id);
        let admission = uuid::Uuid::new_v4().to_string();
        recent.slots.push_back(RecentSlot {
            agent_id: snapshot.session_id.clone(),
            runtime: Arc::downgrade(runtime),
            watch: Arc::downgrade(&snapshot.watch_state),
            scope,
            request_scope: recent_request_scope(snapshot)?,
            watch_cursor: None,
            admission: admission.clone(),
            issued: 1,
            published: false,
            version: digest(b"empty"),
            headers: Vec::new(),
            coverage: Vec::new(),
            charge: 1024
                + snapshot.session_id.len()
                + 100
                + serde_json::to_vec(&native_source)
                    .map_err(io::Error::other)?
                    .len(),
            native_source,
        });
        trim_recent_slots(&mut recent);
        Ok((admission, 1))
    }

    pub(crate) fn abandon_recent(&self, receipt: &(String, u64)) {
        if let Ok(mut recent) = self.recent.lock() {
            recent
                .slots
                .retain(|slot| slot.admission != receipt.0 || slot.issued != receipt.1);
        }
    }

    pub(crate) fn has_recent_native(
        &self,
        snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
        runtime: &Arc<Mutex<String>>,
    ) -> bool {
        self.recent.lock().is_ok_and(|recent| {
            recent.slots.iter().any(|slot| {
                slot.agent_id == snapshot.session_id
                    && slot
                        .native_source
                        .as_ref()
                        .is_some_and(|source| source.is_database())
                    && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                    && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
            })
        })
    }

    /// Selection dispatch performs no source IO or body decoding. A pending
    /// request is released only after its actual blocking job joins.
    pub(crate) fn request_recent(
        &self,
        snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
        runtime: &Arc<Mutex<String>>,
    ) -> io::Result<Option<String>> {
        if snapshot.session_id.len() > 256 {
            return Err(io::Error::other("recent agent identity exceeds byte limit"));
        }
        let scope = recent_request_scope(snapshot)?;
        let cursor = snapshot
            .watch_state
            .try_lock()
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "recent watch busy"))?
            .latest_cursor();
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if recent.global_boundaries > 0 || recent.boundaries.contains_key(&snapshot.session_id) {
            return Ok(None);
        }
        if recent.pending.iter().any(|pending| {
            pending.agent_id == snapshot.session_id
                && pending.runtime.ptr_eq(&Arc::downgrade(runtime))
                && pending.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
        }) {
            return Ok(None);
        }
        let database_provider = matches!(snapshot.provider.as_str(), "opencode" | "antigravity");
        if !database_provider
            && recent.slots.iter().any(|slot| {
                slot.agent_id == snapshot.session_id
                    && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                    && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
                    && slot.request_scope == scope
                    && slot.watch_cursor.as_deref() == Some(cursor.as_str())
            })
        {
            return Ok(None);
        }
        if recent.pending.len() >= 2 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "recent producer capacity busy",
            ));
        }
        let nonce = uuid::Uuid::new_v4().to_string();
        recent.pending.push(PendingRecent {
            agent_id: snapshot.session_id.clone(),
            runtime: Arc::downgrade(runtime),
            watch: Arc::downgrade(&snapshot.watch_state),
            nonce: nonce.clone(),
        });
        trim_recent_slots(&mut recent);
        Ok(Some(nonce))
    }

    pub(crate) fn finish_recent_request(&self, nonce: &str) {
        if let Ok(mut recent) = self.recent.lock() {
            recent.pending.retain(|pending| pending.nonce != nonce);
        }
    }

    pub(crate) fn note_recent_watch(
        &self,
        receipt: &(String, u64),
        cursor: String,
    ) -> io::Result<()> {
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if let Some(slot) = recent
            .slots
            .iter_mut()
            .find(|slot| slot.admission == receipt.0 && slot.issued == receipt.1)
        {
            slot.watch_cursor = Some(cursor);
        }
        Ok(())
    }

    /// The normal publisher supplies original mapper identities. The cache
    /// never derives canonical identities from preview IDs or body text.
    pub(crate) fn note_recent_coverage(
        &self,
        receipt: &(String, u64),
        tags: &HashMap<String, (String, String)>,
    ) -> io::Result<()> {
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if let Some(slot) = recent
            .slots
            .iter_mut()
            .find(|slot| slot.admission == receipt.0 && slot.issued == receipt.1)
        {
            let mut coverage = Vec::new();
            for header in &slot.headers {
                let mut matching = tags.iter().filter(|(_, (key, version))| {
                    header.metadata["chat_recent_key"].as_str() == Some(key.as_str())
                        && header.metadata["chat_recent_version"].as_str() == Some(version.as_str())
                });
                let Some((id, (key, version))) = matching.next() else {
                    return Ok(());
                };
                if matching.next().is_some() {
                    return Ok(());
                }
                if id.len() > 512 {
                    return Ok(());
                }
                coverage.push(RecentCoverage {
                    event_id: id.clone(),
                    key: key.clone(),
                    version: version.clone(),
                    native_key: None,
                });
            }
            let bytes = serde_json::to_vec(&coverage)
                .map_err(io::Error::other)?
                .len();
            slot.charge = slot.charge.saturating_sub(
                serde_json::to_vec(&slot.coverage)
                    .map_err(io::Error::other)?
                    .len(),
            ) + bytes;
            slot.coverage = coverage;
        }
        trim_recent_slots(&mut recent);
        Ok(())
    }

    pub(crate) fn owns_recent_revision(&self, revision: Option<&str>) -> bool {
        self.recent.lock().is_ok_and(|recent| {
            revision.is_some_and(|revision| {
                revision.starts_with(&format!("recent:{}:", recent.revision_owner))
            })
        })
    }

    /// Only the still-current producer token may publish. No provider IO or
    /// archive/projection work lock is held while touching this short mutex.
    pub(crate) fn publish_recent(
        &self,
        snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
        runtime: &Arc<Mutex<String>>,
        receipt: &(String, u64),
        headers: Vec<AgentChatEvent>,
    ) -> io::Result<bool> {
        let (admission, issued) = receipt;
        let native_source = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?
            .slots
            .iter()
            .find(|slot| slot.admission == *admission && slot.issued == *issued)
            .and_then(|slot| slot.native_source.clone());
        if native_source
            .as_ref()
            .is_some_and(|source| !source.valid().unwrap_or(false))
        {
            return Ok(false);
        }
        let (scope, _) = recent_scope(snapshot)?;
        if headers.len() > PAGE_ROWS
            || headers
                .iter()
                .map(|event| &event.id)
                .collect::<BTreeSet<_>>()
                .len()
                != headers.len()
            || headers.iter().any(|event| {
                event.id.is_empty()
                    || event.session_id != snapshot.session_id
                    || event.provider != snapshot.provider
                    || event
                        .text
                        .as_ref()
                        .is_some_and(|text| text.len() > PREVIEW_BYTES)
            })
        {
            return Err(io::Error::other("invalid recent header cohort"));
        }
        let bytes = serde_json::to_vec(&headers).map_err(io::Error::other)?;
        let coverage = if native_source
            .as_ref()
            .is_some_and(|source| source.is_database())
        {
            headers
                .iter()
                .map(|header| {
                    let key = header.metadata["chat_recent_key"]
                        .as_str()
                        .unwrap_or_default();
                    let native_key = key
                        .strip_prefix(&format!("{admission}:"))
                        .filter(|key| {
                            key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                        .ok_or_else(|| io::Error::other("invalid native recent key"))?
                        .to_owned();
                    let version = header.metadata["chat_recent_version"]
                        .as_str()
                        .ok_or_else(|| io::Error::other("missing native recent version"))?
                        .to_owned();
                    Ok(RecentCoverage {
                        event_id: String::new(),
                        key: key.to_owned(),
                        version,
                        native_key: Some(native_key),
                    })
                })
                .collect::<io::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let charge = bytes.len()
            + scope.len()
            + snapshot.session_id.len()
            + admission.len()
            + 1024
            + serde_json::to_vec(&native_source)
                .map_err(io::Error::other)?
                .len()
            + serde_json::to_vec(&coverage)
                .map_err(io::Error::other)?
                .len();
        if charge > IPC_BYTES {
            return Ok(false);
        }
        let version = digest(&bytes);
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if recent.global_boundaries > 0 || recent.boundaries.contains_key(&snapshot.session_id) {
            return Ok(false);
        }
        let Some(slot) = recent.slots.iter_mut().find(|slot| {
            slot.agent_id == snapshot.session_id
                && slot.admission == *admission
                && slot.issued == *issued
                && slot.scope == scope
                && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
        }) else {
            return Ok(false);
        };
        slot.headers = headers;
        slot.coverage = coverage;
        slot.version = version;
        slot.charge = charge;
        slot.published = true;
        trim_recent_slots(&mut recent);
        Ok(true)
    }

    /// Move bounded policy/native metadata reads off the async command thread.
    /// The returned closure owns only the cache and actual runtime identities.
    pub(crate) fn recent_reader(
        &self,
        snapshot: crate::commands::chat::AgentArchiveCaptureSnapshot,
        runtime: Arc<Mutex<String>>,
        revision: Option<String>,
        limit: usize,
    ) -> impl FnOnce() -> io::Result<Option<AgentChatPage>> + Send + 'static {
        let cache = self.recent.clone();
        move || {
            if !(1..=PAGE_ROWS).contains(&limit) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid recent limit",
                ));
            }
            let native_source = cache
                .lock()
                .map_err(|_| io::Error::other("recent owner poisoned"))?
                .slots
                .iter()
                .find(|slot| {
                    slot.agent_id == snapshot.session_id
                        && slot.runtime.ptr_eq(&Arc::downgrade(&runtime))
                        && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
                })
                .and_then(|slot| slot.native_source.clone());
            let source_valid = native_source
                .as_ref()
                .is_none_or(|source| source.valid().unwrap_or(false));
            let scope = recent_scope(&snapshot);
            let recent = cache
                .lock()
                .map_err(|_| io::Error::other("recent owner poisoned"))?;
            let prefix = format!("recent:{}:", recent.revision_owner);
            let previous_preview = revision
                .as_deref()
                .is_some_and(|value| value.starts_with(&prefix));
            let blocked = !source_valid
                || recent.global_boundaries > 0
                || recent.boundaries.contains_key(&snapshot.session_id);
            let valid = recent.slots.iter().position(|slot| {
                slot.agent_id == snapshot.session_id
                    && slot.published
                    && slot.native_source == native_source
                    && !blocked
                    && scope.as_ref().is_ok_and(|(scope, _)| *scope == slot.scope)
                    && slot.runtime.ptr_eq(&Arc::downgrade(&runtime))
                    && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
            });
            let mut coverage = Vec::new();
            let (admission, token, headers, progress) = if let Some(index) = valid {
                let slot = &recent.slots[index];
                if slot.headers.is_empty() && !previous_preview {
                    return Ok(None);
                }
                coverage = slot.coverage.clone();
                let token = format!("{prefix}{}:{}:{limit}", slot.admission, slot.version);
                let headers = slot
                    .headers
                    .iter()
                    .skip(slot.headers.len().saturating_sub(limit))
                    .cloned()
                    .collect();
                (
                    Some(slot.admission.clone()),
                    token,
                    headers,
                    if slot.headers.is_empty() {
                        "no_recent"
                    } else {
                        "provisional"
                    },
                )
            } else {
                // A delayed reader from another incarnation must not erase a
                // newer owner's slot. Only producer/boundary paths retire it.
                if !previous_preview && !blocked {
                    return Ok(None);
                }
                (
                    None,
                    format!("{prefix}retired"),
                    Vec::new(),
                    if blocked {
                        "source_changed"
                    } else if scope.is_err() {
                        "source_unavailable"
                    } else {
                        "no_recent"
                    },
                )
            };
            let unchanged = admission.is_some() && revision.as_deref() == Some(token.as_str());
            let mut page = AgentChatPage {
                session_id: snapshot.session_id.clone(),
                conversation_id: None,
                generation: None,
                source_epoch: None,
                recent_admission: admission,
                revision: token,
                events: if unchanged { Vec::new() } else { headers },
                next_before: None,
                unchanged,
                reset: blocked || (previous_preview && !unchanged),
                older_page: false,
                progress: progress.into(),
                aliases: Vec::new(),
                removed_ids: Vec::new(),
                detail: None,
                revalidated_ids: None,
                bytes_read: scope.as_ref().map(|(_, bytes)| *bytes).unwrap_or(0),
                records_decoded: 0,
            };
            drop(recent);
            if page.recent_admission.is_some() {
                let canonical = recent_canonical_covers(&snapshot, &coverage);
                // Validate again after copying the bounded cohort. The final
                // cache check linearizes this response against privacy guards
                // and newer publications without holding a mutex over IO.
                let after = recent_scope(&snapshot);
                let source_valid = native_source
                    .as_ref()
                    .is_none_or(|source| source.valid().unwrap_or(false));
                page.bytes_read += after.as_ref().map(|(_, bytes)| *bytes).unwrap_or(0);
                let current = cache
                    .lock()
                    .map_err(|_| io::Error::other("recent owner poisoned"))?;
                let valid = source_valid
                    && current.global_boundaries == 0
                    && !current.boundaries.contains_key(&snapshot.session_id)
                    && current.slots.iter().any(|slot| {
                        slot.agent_id == snapshot.session_id
                            && slot.published
                            && slot.native_source == native_source
                            && after.as_ref().is_ok_and(|(scope, _)| *scope == slot.scope)
                            && slot.runtime.ptr_eq(&Arc::downgrade(&runtime))
                            && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
                            && page.recent_admission.as_deref() == Some(slot.admission.as_str())
                            && page.revision
                                == format!("{prefix}{}:{}:{limit}", slot.admission, slot.version)
                    });
                if !valid {
                    page.events.clear();
                    page.recent_admission = None;
                    page.reset = true;
                    page.unchanged = false;
                    page.revision = format!("{prefix}retired");
                    page.progress = "source_changed".into();
                } else if canonical.as_ref().is_ok_and(|(covered, _)| *covered) {
                    // The caller performs its ordinary canonical read and
                    // explicitly resets an incoming owner-preview revision.
                    return Ok(None);
                }
                page.bytes_read += canonical.map(|(_, bytes)| bytes).unwrap_or(0);
            }
            Ok(Some(page))
        }
    }
}

fn recent_canonical_covers(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    coverage: &[RecentCoverage],
) -> io::Result<(bool, usize)> {
    if coverage.is_empty() {
        return Ok((false, 0));
    }
    let Some(reference) = head(&snapshot.session_id)? else {
        return Ok((false, 0));
    };
    let mut store = store(&snapshot.session_id)?;
    let published: Head = store.read(&reference)?;
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    if published.agent_id != snapshot.session_id
        || published.source_key != context.provider_source_key
        || !cold_bootstrap::scope_matches(&published, &context)
    {
        return Ok((false, store.bytes));
    }
    // Identity and membership seeks share immutable nodes across the cohort.
    // Keep reuse local to this proof and retain the Store's hard read limits.
    let mut seeks = SeekCache::default();
    for binding in coverage {
        let identity = binding.native_key.as_ref().map_or_else(
            || {
                if snapshot.provider == "mock" {
                    // Verified mirror handoff records this namespace rather
                    // than transferring the generic Watch identity. Avoid a
                    // guaranteed extra tree walk for every mirrored header.
                    digest(format!("mock-watch:{}", binding.event_id).as_bytes())
                } else {
                    digest(binding.event_id.as_bytes())
                }
            },
            |key| recent_native_index_key(key),
        );
        let row_ref = store.get_reusing_nodes(&published.identities, &identity, &mut seeks)?;
        let row_ref =
            if row_ref.is_none() && binding.native_key.is_none() && snapshot.provider == "mock" {
                store.get_reusing_nodes(
                    &published.identities,
                    &digest(binding.event_id.as_bytes()),
                    &mut seeks,
                )?
            } else {
                row_ref
            };
        let Some(row_ref) = row_ref else {
            return Ok((false, store.bytes));
        };
        let row: Row = store.read(&row_ref)?;
        let bound = if let Some(native_key) = &binding.native_key {
            row.event.metadata["chat_recent_native_key"].as_str() == Some(native_key.as_str())
                && row.event.metadata["chat_recent_native_version"].as_str()
                    == Some(binding.version.as_str())
        } else {
            (row.event.id == binding.event_id
                || (row.event.provider == "mock"
                    && row.event.metadata["mock_watch_alias"].as_str()
                        == Some(binding.event_id.as_str())
                    && row.event.metadata["chat_source_ref"].is_string()))
                && row.event.metadata["chat_recent_key"].as_str() == Some(binding.key.as_str())
                && row.event.metadata["chat_recent_version"].as_str()
                    == Some(binding.version.as_str())
        };
        if row.agent_id != snapshot.session_id
            || row.conversation_id != published.conversation_id
            || row.event.provider != snapshot.provider
            || !bound
            || store
                .get_reusing_nodes(&published.root, &row.key, &mut seeks)?
                .as_deref()
                != Some(row_ref.as_str())
        {
            return Ok((false, store.bytes));
        }
    }
    Ok((
        head(&snapshot.session_id)?.as_deref() == Some(reference.as_str()),
        store.bytes,
    ))
}

fn recent_native_index_key(key: &str) -> String {
    digest(format!("recent-native:{key}").as_bytes())
}

fn trim_recent_slots(recent: &mut RecentSlots) {
    let controls =
        recent.boundaries.len() + usize::from(recent.global_boundaries > 0) + recent.pending.len();
    let control_bytes = recent
        .boundaries
        .keys()
        .map(|key| key.len() + 128)
        .sum::<usize>()
        + recent
            .pending
            .iter()
            .map(|pending| pending.agent_id.len() + pending.nonce.len() + 128)
            .sum::<usize>()
        + 4096;
    while recent
        .slots
        .iter()
        .map(|slot| 1 + slot.headers.len())
        .sum::<usize>()
        + controls
        > 512
        || recent.slots.iter().map(|slot| slot.charge).sum::<usize>() + control_bytes > 1024 * 1024
    {
        recent.slots.pop_front();
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionWorkTestSnapshot {
    pub(crate) agent_id: String,
    pub(crate) conversation_id: String,
    pub(crate) source_key: Option<String>,
    pub(crate) source_epoch: Option<String>,
    pub(crate) head_generation: String,
    pub(crate) head_source_epoch: Option<String>,
    pub(crate) head_progress: String,
    pub(crate) committed_output_bytes: u64,
    pub(crate) row_count: usize,
    pub(crate) todo_count: usize,
    pub(crate) body_jobs: usize,
    pub(crate) remaining_body_bytes: u64,
    pub(crate) candidate_events: usize,
    pub(crate) logical_scalars: serde_json::Value,
}

impl std::fmt::Debug for ProjectionOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectionOwner").finish_non_exhaustive()
    }
}

#[cfg(test)]
impl ProjectionOwner {
    pub(crate) fn test_work_snapshot(
        &self,
        agent_id: &str,
    ) -> io::Result<Option<ProjectionWorkTestSnapshot>> {
        let works = self
            .work
            .lock()
            .map_err(|_| io::Error::other("chat projection owner poisoned"))?;
        let Some(work) = works.get(agent_id) else {
            return Ok(None);
        };
        let logical = serde_json::to_value(&work.head.logical).map_err(io::Error::other)?;
        let remaining_body_bytes = work.bodies.iter().try_fold(0u64, |total, job| {
            let remaining = job
                .source
                .length()?
                .checked_sub(job.offset)
                .ok_or_else(|| io::Error::other("chat body offset exceeds source length"))?;
            total
                .checked_add(remaining)
                .ok_or_else(|| io::Error::other("chat body work length overflow"))
        })?;
        let logical_scalars = serde_json::json!({
            "buckets": logical
                .get("buckets")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "pending": logical
                .get("pending")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "claims": logical
                .get("claims")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "before": logical
                .get("before")
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
        Ok(Some(ProjectionWorkTestSnapshot {
            agent_id: work.candidate.context.agent_id.clone(),
            conversation_id: work.candidate.conversation_id.clone(),
            source_key: work.candidate.context.provider_source_key.clone(),
            source_epoch: work.candidate.source_epoch.clone(),
            head_generation: work.head.generation.clone(),
            head_source_epoch: work.head.source_epoch.clone(),
            head_progress: work.head.progress.clone(),
            committed_output_bytes: work.head.committed_output_bytes,
            row_count: work.head.row_count,
            todo_count: work.todo.len(),
            body_jobs: work.bodies.len(),
            remaining_body_bytes,
            candidate_events: work.candidate.events.len(),
            logical_scalars,
        }))
    }

    pub(crate) fn recent_pending_for_test(&self) -> usize {
        self.recent.lock().unwrap().pending.len()
    }

    pub(crate) fn begin_recent(
        &self,
        snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
        runtime: &Arc<Mutex<String>>,
    ) -> io::Result<(String, u64)> {
        self.begin_recent_with_source(snapshot, runtime, None)
    }

    pub(crate) fn retire_recent(&self, agent_id: &str) -> io::Result<()> {
        self.recent
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?
            .slots
            .retain(|slot| slot.agent_id != agent_id);
        Ok(())
    }

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

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceIndexTestSnapshot {
    pub(crate) generation: String,
    pub(crate) epoch: String,
    pub(crate) progress: String,
    pub(crate) complete: bool,
    pub(crate) scalars: serde_json::Value,
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
        "chat_recent_key",
        "chat_recent_version",
        "chat_recent_native_key",
        "chat_recent_native_version",
        "mock_watch_alias",
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
        let source_coverage =
            super::chat_logical_index::mock_canonical_coverage(&candidate.context)?;
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
            source_coverage,
            parent: None,
            changes: Vec::new(),
            recent_start: candidate.events.len().saturating_sub(PAGE_ROWS),
            row_count: candidate.events.len(),
            recent_page: None,
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
        // Replaying an unchanged settled capture must preserve the opaque
        // revision already held by a concurrent bounded source-renewal phase.
        // Source-proof publishers retain their existing qualification path.
        if source_proof.is_none()
            && works.get(&agent_id).is_some_and(|old| {
                old.source_proof.is_none()
                    && old.head.progress == "ready"
                    && old.todo.is_empty()
                    && old.bodies.is_empty()
                    && !old.head.logical.pending()
                    && !old.head.logical.narrative_pending()
                    && old.head.conversation_id == head.conversation_id
                    && old.head.source_key == head.source_key
                    && old.head.source_epoch == head.source_epoch
                    && old.head.capture_stamp == head.capture_stamp
                    && old.head.source_coverage == head.source_coverage
                    && old.head.committed_output_bytes == head.committed_output_bytes
                    && old.candidate.context == candidate.context
                    && old.candidate.generated_input_bindings == candidate.generated_input_bindings
                    && old.signatures == signatures
            })
        {
            return Ok(());
        }
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
        head.recent_page = None;
        head.recent_page = packed_recent::prepare(&mut store, &head)?;
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
        let _recent_boundary = self.recent_boundary(Some(agent_id))?;
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
        head.recent_page = None;
        head.recent_page = packed_recent::prepare(&mut store, &head)?;
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
    if row.event.provider == "mock" && row.event.metadata["chat_source_ref"].is_string() {
        if let Some(alias) = row.event.metadata["mock_watch_alias"]
            .as_str()
            .filter(|alias| alias.len() <= 256)
        {
            head.identities = Some(store.insert(
                &head.identities,
                &digest(format!("mock-watch:{alias}").as_bytes()),
                &reference,
            )?);
        }
    }
    if let Some(native) = row.event.metadata["chat_recent_native_key"]
        .as_str()
        .filter(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        if (row.event.source.as_deref() == Some("opencode_db") && row.event.provider == "opencode")
            || (row.event.source.as_deref() == Some("conversation_database")
                && row.event.provider == "antigravity")
        {
            head.identities = Some(store.insert(
                &head.identities,
                &recent_native_index_key(native),
                &reference,
            )?);
        }
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
    page_limited(store, head_ref, head, before, PAGE_ROWS)
}

/// A recent target never splits a physical frame or certified member component.
pub(super) fn recent_window_start(events: &[AgentChatEvent], limit: usize) -> usize {
    fn linked(a: &AgentChatEvent, b: &AgentChatEvent) -> bool {
        let same_frame = a.metadata["chat_source_epoch"].as_str().is_some()
            && a.metadata["chat_source_epoch"] == b.metadata["chat_source_epoch"]
            && a.metadata["chat_source_start"].as_u64().is_some()
            && a.metadata["chat_source_start"] == b.metadata["chat_source_start"]
            && a.metadata["chat_source_end"].as_u64().is_some()
            && a.metadata["chat_source_end"] == b.metadata["chat_source_end"];
        let contains = |event: &AgentChatEvent, id: &str| {
            event.id == id
                || event.metadata["chat_source_ref"].as_str() == Some(id)
                || event.metadata["chat_display_member_ids"]
                    .as_array()
                    .is_some_and(|members| members.iter().any(|member| member.as_str() == Some(id)))
        };
        same_frame
            || contains(a, &b.id)
            || contains(b, &a.id)
            || a.metadata["chat_display_member_ids"]
                .as_array()
                .is_some_and(|members| {
                    members
                        .iter()
                        .filter_map(|member| member.as_str())
                        .any(|member| contains(b, member))
                })
    }
    let mut start = events.len().saturating_sub(limit);
    while let Some(earlier) = events[..start].iter().position(|event| {
        events[start..]
            .iter()
            .any(|selected| linked(event, selected))
    }) {
        start = earlier;
    }
    start
}

fn page_limited(
    store: &mut Store,
    head_ref: &str,
    head: &Head,
    before: Option<&str>,
    limit: usize,
) -> io::Result<(Vec<AgentChatEvent>, Option<String>)> {
    let (entries, has_older) =
        if let Some(reference) = head.recent_page.as_deref().filter(|_| before.is_none()) {
            let packed = packed_recent::read(store, reference, head)?;
            (packed.entries, packed.has_older)
        } else {
            let physical = store.page(&head.root, before, PAGE_ROWS + 1)?;
            let has_older = physical.len() > PAGE_ROWS;
            let entries = physical
                .into_iter()
                .take(PAGE_ROWS)
                .map(|(key, reference)| {
                    let row = store.read(&reference)?;
                    Ok(packed_recent::PackedEntry {
                        key,
                        reference,
                        row,
                    })
                })
                .collect::<io::Result<Vec<_>>>()?;
            (entries, has_older)
        };
    let mut events = Vec::new();
    let mut last = None;
    let mut emitted_keys = Vec::new();
    for packed_recent::PackedEntry {
        key,
        reference,
        row,
    } in entries
    {
        if row.agent_id != head.agent_id || row.conversation_id != head.conversation_id {
            return Err(io::Error::other("foreign chat row"));
        }
        last = Some(key);
        emitted_keys.push(last.clone().expect("a row key was just selected"));
        let mut event = display_event(row, &reference);
        if let Some(detail) = event.metadata["chat_detail_ref"].as_str() {
            event.metadata["chat_detail_ref"] = json!(format!("{head_ref}:{detail}"));
        }
        events.push(event);
    }
    events.reverse();
    emitted_keys.reverse();
    let start = recent_window_start(&events, limit);
    if start > 0 {
        last = emitted_keys.get(start).cloned();
        events.drain(..start);
    }
    let next = if has_older || start > 0 || head.progress == "indexing" {
        last.as_deref()
            .or(before)
            .map(|key| format!("{head_ref}:{key}"))
    } else {
        None
    };
    Ok((events, next))
}

/// Bind a reduced mixed window to the first physical row actually emitted.
/// A new raw suffix may have no canonical row yet. In that case Older waits
/// for publication rather than inheriting a cursor past omitted headers.
fn canonical_recent_frontier(
    store: &mut Store,
    reference: &str,
    head: &Head,
    event: &AgentChatEvent,
) -> io::Result<Option<String>> {
    let Some(source) = event.metadata["chat_source_ref"].as_str() else {
        return Ok(None);
    };
    let Some(row_ref) = store.get(&head.identities, &digest(source.as_bytes()))? else {
        return Ok(None);
    };
    let row: Row = store.read(&row_ref)?;
    if row.agent_id != head.agent_id
        || row.conversation_id != head.conversation_id
        || row.event.metadata["chat_source_ref"].as_str() != Some(source)
        || row.event.metadata["chat_source_epoch"] != event.metadata["chat_source_epoch"]
        || row.event.metadata["chat_source_start"] != event.metadata["chat_source_start"]
        || row.event.metadata["chat_source_end"] != event.metadata["chat_source_end"]
        || store.get(&head.root, &row.key)?.as_deref() != Some(row_ref.as_str())
    {
        return Err(io::Error::other("foreign recent chat frontier"));
    }
    Ok(
        (!store.page(&head.root, Some(&row.key), 1)?.is_empty() || head.progress == "indexing")
            .then(|| format!("{reference}:{}", row.key)),
    )
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

fn archived_codex_commentary_text(event: &AgentChatEvent) -> Option<&str> {
    let text = event
        .text
        .as_deref()
        .filter(|text| !text.trim().is_empty() && text.len() < PREVIEW_BYTES)?;
    if event.metadata["chat_body_pending"] == true
        || event.metadata["chat_body_unavailable"] == true
        || event.metadata["codex_display_text_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes != text.len() as u64)
    {
        return None;
    }
    Some(text)
}

fn archived_codex_commentary_fields_match(
    archived: &AgentChatEvent,
    native: &AgentChatEvent,
    source: &str,
    raw_types: &[&str],
    companion_text: &str,
) -> bool {
    if archived.session_id != native.session_id
        || archived.provider != "codex"
        || archived.kind != wardian_core::models::chat::AgentChatEventKind::Message
        || archived.role != Some(wardian_core::models::chat::AgentChatRole::Assistant)
        || archived.source.as_deref() != Some(source)
        || archived.turn_id != native.turn_id
        || archived.metadata["chat_source_ref"].is_string()
        || archived.metadata["generated"] == true
        || archived
            .metadata
            .get("provider_log")
            .is_some_and(|value| value != &json!(true))
        || archived.metadata["raw_type"]
            .as_str()
            .is_some_and(|raw_type| !raw_types.contains(&raw_type))
        || !archived
            .text
            .as_deref()
            .is_some_and(|text| text == companion_text)
        || archived_codex_commentary_text(archived) != Some(companion_text)
        || !archived
            .text
            .as_deref()
            .zip(native.text.as_deref())
            .is_some_and(|(archived, current)| archived == current)
    {
        return false;
    }
    [
        "provider_session_id",
        "conversation_archive_id",
        "provider_turn_id",
        "provider_phase",
        "log_path",
        "source_path",
        "chat_source_epoch",
        "chat_source_admission",
    ]
    .into_iter()
    .all(|key| {
        archived
            .metadata
            .get(key)
            .filter(|value| !value.is_null())
            .is_none_or(|value| native.metadata.get(key) == Some(value))
    })
}

fn archived_codex_commentary_matches(
    archived: &AgentChatEvent,
    native: &AgentChatEvent,
    source: &str,
    raw_types: &[&str],
    companion_text: &str,
) -> bool {
    !archived.metadata["chat_display_member_ids"].is_array()
        && archived_codex_commentary_fields_match(
            archived,
            native,
            source,
            raw_types,
            companion_text,
        )
}

/// A projected logical group has a different public ID from its physical
/// members. Accept overlap only when the current SourcePage certifies exactly
/// this A/B pair and the bounded relation contains no other native identity.
fn projected_codex_commentary_group_members(
    projected: &AgentChatEvent,
    source_page: &super::chat_source_index::SourcePage,
    live_id: &str,
    completed_id: &str,
) -> Option<(String, Vec<String>)> {
    let physical_id = projected
        .metadata
        .get("chat_display_physical_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty())?;
    let members = projected
        .metadata
        .get("chat_display_member_ids")
        .and_then(serde_json::Value::as_array)?;
    if !(3..=4).contains(&members.len()) {
        return None;
    }
    let members = members
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    let member_set = members.iter().copied().collect::<BTreeSet<_>>();
    if members.iter().any(|id| id.trim().is_empty())
        || member_set.len() != members.len()
        || !member_set.contains(live_id)
        || !member_set.contains(completed_id)
        || !member_set.contains(physical_id)
    {
        return None;
    }

    let mut native_ids = BTreeSet::new();
    for event in &source_page.events {
        native_ids.insert(event.id.as_str());
        if let Some(id) = event.metadata["chat_source_ref"].as_str() {
            native_ids.insert(id);
        }
        if let Some(id) = event.metadata["chat_display_physical_id"].as_str() {
            native_ids.insert(id);
        }
        if let Some(ids) = event.metadata["chat_display_member_ids"].as_array() {
            native_ids.extend(ids.iter().filter_map(serde_json::Value::as_str));
        }
    }
    if projected.id == live_id
        || projected.id == completed_id
        || native_ids.contains(projected.id.as_str())
        || members
            .iter()
            .any(|id| *id != live_id && *id != completed_id && native_ids.contains(*id))
    {
        return None;
    }

    Some((
        physical_id.to_owned(),
        members.into_iter().map(str::to_owned).collect(),
    ))
}

fn certified_codex_commentary_member(
    event: &AgentChatEvent,
    source_ref: &str,
    live_id: &str,
    completed_id: &str,
    source: &str,
    raw_types: &[&str],
) -> bool {
    event.id == live_id
        && event.provider == "codex"
        && event.kind == wardian_core::models::chat::AgentChatEventKind::Message
        && event.role == Some(wardian_core::models::chat::AgentChatRole::Assistant)
        && event.source.as_deref() == Some(source)
        && event.metadata["provider_log"] == true
        && event.metadata["provider_phase"] == "commentary"
        && event.metadata["raw_type"]
            .as_str()
            .is_some_and(|raw_type| raw_types.contains(&raw_type))
        && event.metadata["chat_source_ref"].as_str() == Some(source_ref)
        && event.metadata["chat_display_physical_id"].as_str() == Some(source_ref)
        && event.metadata["chat_source_start"].as_u64().is_some()
        && event.metadata["chat_source_end"].as_u64().is_some()
        && event.metadata["chat_display_member_ids"]
            .as_array()
            .is_some_and(|members| {
                members.len() == 2
                    && members
                        .iter()
                        .filter_map(|member| member.as_str())
                        .collect::<BTreeSet<_>>()
                        == BTreeSet::from([live_id, completed_id])
            })
}

fn reconcile_archived_codex_commentary_pairs(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    page: &mut AgentChatPage,
    source_page: &super::chat_source_index::SourcePage,
    logical_admitted: bool,
    source_scope_valid: bool,
    source_pairs: &[(String, String)],
) {
    if page.reset
        || source_pairs.is_empty()
        || !super::chat_source_index::current_complete(snapshot, source_page)
    {
        return;
    }

    let mut candidates = Vec::new();
    for (live_id, completed_id) in source_pairs {
        let live_rows = source_page
            .events
            .iter()
            .filter(|event| {
                certified_codex_commentary_member(
                    event,
                    live_id,
                    live_id,
                    completed_id,
                    "event_msg",
                    &["agent_message", "assistant_message"],
                )
            })
            .collect::<Vec<_>>();
        let completed_rows = source_page
            .events
            .iter()
            .filter(|event| {
                certified_codex_commentary_member(
                    event,
                    completed_id,
                    live_id,
                    completed_id,
                    "response_item",
                    &["message"],
                )
            })
            .collect::<Vec<_>>();
        let (Some(live), Some(completed)) = (
            live_rows.first().filter(|_| live_rows.len() == 1),
            completed_rows.first().filter(|_| completed_rows.len() == 1),
        ) else {
            continue;
        };
        if !crate::providers::chat_transcript::codex_display_pair(live, completed) {
            continue;
        }
        let visible_display = page.events.iter().any(|event| {
            event.id == *live_id
                && event.metadata["chat_source_ref"].is_string()
                && event.metadata["chat_display_member_ids"]
                    .as_array()
                    .is_some_and(|members| {
                        members.len() == 2
                            && members
                                .iter()
                                .filter_map(|member| member.as_str())
                                .collect::<BTreeSet<_>>()
                                == BTreeSet::from([live_id.as_str(), completed_id.as_str()])
                    })
        });
        if !visible_display {
            continue;
        }

        let companion_text = completed.text.as_deref().unwrap_or_default();
        let response_rows = page
            .events
            .iter()
            .filter(|event| {
                archived_codex_commentary_matches(
                    event,
                    completed,
                    "response_item",
                    &["message"],
                    companion_text,
                )
            })
            .collect::<Vec<_>>();
        let projected_rows = if logical_admitted && source_scope_valid {
            page.events
                .iter()
                .filter_map(|event| {
                    let group = projected_codex_commentary_group_members(
                        event,
                        source_page,
                        live_id,
                        completed_id,
                    )?;
                    archived_codex_commentary_fields_match(
                        event,
                        completed,
                        "response_item",
                        &["message"],
                        companion_text,
                    )
                    .then_some((event, group))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let (response, projected_group) =
            match (response_rows.as_slice(), projected_rows.as_slice()) {
                ([response], []) => (*response, None),
                ([], [(response, (physical_id, members))]) => {
                    (*response, Some((physical_id.clone(), members.clone())))
                }
                _ => continue,
            };
        let stream_rows = page
            .events
            .iter()
            .filter(|event| {
                archived_codex_commentary_matches(
                    event,
                    live,
                    "event_msg",
                    &["agent_message", "assistant_message"],
                    companion_text,
                )
            })
            .collect::<Vec<_>>();
        let completion_message_id = completed
            .turn_id
            .as_deref()
            .filter(|message_id| !message_id.trim().is_empty());
        if projected_group.is_some()
            && !completion_message_id
                .is_some_and(|message_id| response.turn_id.as_deref() == Some(message_id))
        {
            continue;
        }
        let stream_id = match stream_rows.as_slice() {
            [stream] => Some(stream.id.clone()),
            [] if completion_message_id
                .is_some_and(|message_id| response.turn_id.as_deref() == Some(message_id)) =>
            {
                None
            }
            _ => continue,
        };
        candidates.push((
            live_id.clone(),
            stream_id,
            response.id.clone(),
            projected_group,
        ));
    }

    let mut stream_claims = HashMap::<String, usize>::new();
    let mut response_claims = HashMap::<String, usize>::new();
    let mut projected_group_claims = HashMap::<Vec<String>, usize>::new();
    for (_, stream_id, response_id, projected_group) in &candidates {
        if let Some(stream_id) = stream_id {
            *stream_claims.entry(stream_id.clone()).or_default() += 1;
        }
        *response_claims.entry(response_id.clone()).or_default() += 1;
        if let Some((_, members)) = projected_group {
            let mut group_key = members.to_vec();
            group_key.sort();
            *projected_group_claims.entry(group_key).or_default() += 1;
        }
    }
    let mut archived_ids = BTreeSet::new();
    for (live_id, stream_id, response_id, projected_group) in candidates {
        if stream_id
            .as_ref()
            .is_some_and(|stream_id| stream_claims.get(stream_id) != Some(&1))
            || response_claims.get(&response_id) != Some(&1)
            || projected_group.as_ref().is_some_and(|(_, members)| {
                let mut group_key = members.to_vec();
                group_key.sort();
                projected_group_claims.get(&group_key) != Some(&1)
            })
        {
            continue;
        }

        let mut observation_ids = BTreeSet::new();
        if let Some(stream_id) = stream_id {
            archived_ids.insert(stream_id.clone());
            observation_ids.insert(stream_id);
        }
        archived_ids.insert(response_id.clone());
        observation_ids.insert(response_id);
        if let Some((physical_id, members)) = projected_group {
            observation_ids.insert(physical_id);
            observation_ids.extend(members);
            // The group public ID and its physical members are one certified
            // logical observation. Rewrite prior alias targets before adding
            // the native live ID, so no observation points at both headers.
            for alias in &mut page.aliases {
                if observation_ids.contains(&alias.observation_id)
                    || observation_ids.contains(&alias.canonical_id)
                {
                    alias.canonical_id = live_id.clone();
                }
            }
        }
        for observation_id in observation_ids {
            if observation_id != live_id {
                page.aliases
                    .push(wardian_core::models::chat::AgentChatAlias {
                        observation_id,
                        canonical_id: live_id.clone(),
                    });
            }
        }
    }
    if !archived_ids.is_empty() {
        page.events
            .retain(|event| !archived_ids.contains(&event.id));
    }
}

/// The shared read boundary. Every disk/decode path uses fixed limits, including
/// stale cursors and detail failures. No capture, repair or archive lock occurs.
pub(crate) fn read_with_revalidation_and_limit(
    context: &ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    cursor: Option<&str>,
    revision: Option<&str>,
    detail_ref: Option<&str>,
    revalidate_ids: Option<&[String]>,
    limit: Option<usize>,
) -> io::Result<AgentChatPage> {
    let limit = limit.unwrap_or(PAGE_ROWS);
    if !(1..=PAGE_ROWS).contains(&limit) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid recent Chat limit",
        ));
    }
    let Some(revalidate_ids) = revalidate_ids else {
        return read_with_recent_limit(context, snapshot, cursor, revision, detail_ref, limit);
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

fn read_with_recent_limit(
    context: &ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    cursor: Option<&str>,
    revision: Option<&str>,
    detail_ref: Option<&str>,
    limit: usize,
) -> io::Result<AgentChatPage> {
    let limit = if cursor.is_none() && detail_ref.is_none() {
        limit
    } else {
        PAGE_ROWS
    };
    let window_suffix = format!(":recent:{limit}");
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
    let previous_window_matches = revision
        .and_then(|value| value.split_once('.'))
        .map(|(_, seed)| seed.split_once('~').map_or(seed, |(seed, _)| seed))
        .is_none_or(|seed| {
            if limit == PAGE_ROWS {
                !seed.contains(":recent:")
            } else {
                seed.ends_with(&window_suffix)
            }
        });
    let previous_seed = revision
        .and_then(|value| value.split_once('.'))
        .filter(|(previous, _)| *previous == current.as_deref().unwrap_or("none"))
        .map(|(_, seed)| seed.split_once('~').map_or(seed, |(seed, _)| seed))
        .and_then(|seed| {
            if limit == PAGE_ROWS {
                (!seed.contains(":recent:")).then_some(seed)
            } else {
                seed.strip_suffix(&window_suffix)
            }
        });
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
        crate::commands::chat_recent_seed::read_limited(snapshot, previous_seed, limit)?
    } else {
        crate::commands::chat_recent_seed::Seed {
            revision: "page".into(),
            progress: "ready".into(),
            ..Default::default()
        }
    };
    let seed_token = |seed: &str| {
        if limit == PAGE_ROWS {
            seed.to_owned()
        } else {
            format!("{seed}{window_suffix}")
        }
    };
    let mut token = format!(
        "{}.{}~{}",
        current.as_deref().unwrap_or("none"),
        seed_token(&seed.revision),
        source_pointer.as_deref().unwrap_or("none")
    );
    let mut result = AgentChatPage {
        session_id: context.agent_id.clone(),
        conversation_id: None,
        generation: None,
        source_epoch: None,
        recent_admission: None,
        revision: token.clone(),
        events: Vec::new(),
        next_before: None,
        unchanged: false,
        reset: cold_rejected,
        older_page: false,
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
        result.progress = if seed.revision == "unsupported" {
            "no_recent"
        } else {
            "projection_pending"
        }
        .into();
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
                "oversized_record" | "source_gap" | "source_changed" | "ownership_pending"
            )
        {
            result.progress = published.progress.clone();
        }
    }
    let source_scope_attempted = source_pointer.is_some();
    let mut source_scope_valid = !source_scope_attempted;
    let mut mock_source_progress = None;
    let mut qualified_mock_canonical = false;
    if context.provider == "mock" && snapshot.log_path.is_some() && source_pointer.is_none() {
        mock_source_progress = Some("indexing".to_owned());
    }
    if let Some(reference) = &source_pointer {
        result.bytes_read += (2 * HEAD_BYTES + POLICY_BYTES) as usize;
        result.records_decoded += 2;
        match super::chat_source_index::qualification(snapshot, &mut store, reference) {
            Ok(scope) => {
                qualified_mock_canonical = context.provider == "mock"
                    && current_head.as_ref().is_some_and(|published| {
                        published.progress == "ready"
                            && !published.logical.pending()
                            && !published.logical.narrative_pending()
                            && published.source_epoch.as_deref() == Some(scope.epoch.as_str())
                            && published
                                .source_coverage
                                .as_ref()
                                .is_some_and(|proof| proof.matches(&scope))
                    });
                if context.provider == "mock"
                    && (!scope.complete || (current_head.is_some() && !qualified_mock_canonical))
                {
                    mock_source_progress = Some(
                        if matches!(
                            scope.progress.as_str(),
                            "ready" | "provisional" | "no_admitted_rows"
                        ) {
                            "indexing".to_owned()
                        } else {
                            scope.progress.clone()
                        },
                    );
                }
                source_scope_valid = true;
                result.generation = Some(scope.generation);
                result.source_epoch = Some(scope.epoch);
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
            let revalidated_seed =
                crate::commands::chat_recent_seed::read_limited(snapshot, None, limit)?;
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
                seed_token(&seed.revision),
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
            if matches!(result.progress.as_str(), "ready" | "provisional") {
                if let Some(progress) = &mock_source_progress {
                    result.progress.clone_from(progress);
                }
            }
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
            match super::chat_source_index::page_limited(
                snapshot,
                &mut store,
                reference,
                None,
                previous_source.filter(|source| *source != "none"),
                limit,
            ) {
                Ok(page) => {
                    source_page_pairs = source_page_commentary_pairs(snapshot, &page);
                    source_page_reset = page.reset;
                    result.reset |= page.reset;
                    if !matches!(
                        result.progress.as_str(),
                        "oversized_record" | "source_gap" | "source_changed" | "ownership_pending"
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
            match super::chat_source_index::page_limited(
                snapshot,
                &mut store,
                reference,
                cursor.filter(|_| source_cursor),
                previous_source.filter(|source| *source != "none"),
                limit,
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
                            result.older_page = true;
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
                                    result.older_page = !result.events.is_empty();
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
                                        result.older_page = false;
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
                                    result.older_page = false;
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
                            result.older_page = false;
                        }
                    } else if page.counts_complete {
                        result.events = page.events;
                        result.next_before = page.next_before;
                    } else {
                        // A sparse forward prefix is neither the recent window
                        // nor a valid source-page frontier.
                        result.events.clear();
                        result.next_before = None;
                        result.older_page = false;
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
                result.older_page = true;
            } else {
                result.reset = true;
                (result.events, result.next_before) =
                    page_limited(&mut store, reference, published, None, limit)?;
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
            let delta =
                if refresh_source_projection || limit != PAGE_ROWS || !previous_window_matches {
                    None
                } else {
                    previous
                        .filter(|previous| valid_ref(previous))
                        .map(|previous| changes_since(&mut store, reference, published, previous))
                        .transpose()?
                        .flatten()
                };
            let compatible_delta = delta.is_some();
            let qualified_snapshot = qualified_mock_canonical && revision != Some(token.as_str());
            if let Some(mut delta) = delta.filter(|_| !qualified_snapshot) {
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
                result.reset |= previous.is_some() && !compatible_delta;
                (result.events, result.next_before) =
                    page_limited(&mut store, reference, published, None, limit)?;
            }
        }
        if published.progress != "ready"
            && !matches!(
                result.progress.as_str(),
                "oversized_record" | "source_gap" | "source_changed" | "ownership_pending"
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
        if limit < PAGE_ROWS {
            let start = recent_window_start(&result.events, limit);
            if start > 0 {
                let first = &result.events[start];
                result.next_before =
                    if let (Some(reference), Some(published)) = (&current, &current_head) {
                        canonical_recent_frontier(&mut store, reference, published, first)?
                    } else if let (Some(reference), Some(checkpoint)) =
                        (&source_pointer, &seed.checkpoint)
                    {
                        super::chat_source_index::continuation(
                            snapshot,
                            &mut store,
                            reference,
                            checkpoint,
                            Some(first),
                        )?
                    } else {
                        None
                    };
                result.events.drain(..start);
            }
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
        result.older_page = false;
        if !matches!(
            result.progress.as_str(),
            "oversized_record" | "source_changed" | "ownership_pending"
        ) {
            result.progress = "indexing".into();
        }
        result.unchanged = false;
    }
    if cursor.is_none() && detail_ref.is_none() {
        if let Some(source_page) = seed_source_page.as_ref() {
            reconcile_archived_codex_commentary_pairs(
                snapshot,
                &mut result,
                source_page,
                logical_admitted,
                source_scope_valid,
                &source_page_pairs,
            );
        }
    }
    if detail_ref.is_some() {
        result.older_page = false;
    }
    project_display_result(&mut result);
    // Qualification describes this canonical interval on both fresh and
    // unchanged reads. A source page loaded for validation is independently
    // provisional; it does not demote the canonical rows assembled above.
    if qualified_mock_canonical
        && cursor.is_none()
        && detail_ref.is_none()
        && !defer_source_page_for_unchecked_relation
        && result.progress == "provisional"
    {
        result.progress = "ready".into();
    }
    if detail_ref.is_none() && matches!(result.progress.as_str(), "ready" | "provisional") {
        if let Some(progress) = mock_source_progress {
            result.progress = progress;
        }
    }
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
        recent_admission: None,
        revision: revision.to_owned(),
        events,
        next_before: None,
        unchanged: false,
        reset: canonical_rejected || progress == "source_changed",
        older_page: false,
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
        recent_admission: None,
        revision: revision.to_owned(),
        events,
        next_before: None,
        unchanged: false,
        reset: false,
        older_page: false,
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
pub(crate) use tests::{read, read_with_revalidation};

#[cfg(test)]
#[path = "chat_read_tests.rs"]
mod tests;
