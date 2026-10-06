//! Bounded, single-writer lifecycle folding for validated Codex rollouts.
//!
//! The caller must serialize watcher and periodic observations through one
//! [`LifecycleObserver`], validate the source's parent ancestry before creating
//! a [`ValidatedCodexSource`], publish the returned state, then commit the
//! [`LifecycleStage`]. Dropping a stage retains its candidate checkpoint as
//! uncommitted; [`LifecycleObserver::retry_pending`] reuses it without rescanning.

use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::SystemTime;
use wardian_core::temporary_workers::TemporaryWorkerState;

pub const CODEX_LIFECYCLE_PARSER_VERSION: u32 = 1;
pub const CODEX_LIFECYCLE_BYTES_PER_PASS: u64 = 256 * 1024;

const MAX_TRACKED_SOURCES: usize = 4096;
const MAX_HEADER_BYTES: usize = 256 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHED_PARTIAL_BYTES: usize = 16 * 1024 * 1024;
const IO_CHUNK_BYTES: usize = 8 * 1024;

/// A source whose path and parent chain were validated by the caller.
///
/// `canonical_path` must be canonical. `from_verified_ancestry` is intended to
/// be called only for descendants returned by Wardian's existing Codex
/// ownership validation. The rollout header is independently checked against
/// this thread and parent identity before any lifecycle state can be published.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedCodexSource {
    canonical_path: PathBuf,
    native_thread_id: String,
    parent_thread_id: Option<String>,
    activity_epoch: Option<(String, LifecycleActivityBaseline)>,
}

impl ValidatedCodexSource {
    pub fn from_verified_ancestry(
        canonical_path: PathBuf,
        native_thread_id: impl Into<String>,
        parent_thread_id: Option<String>,
    ) -> Self {
        Self {
            canonical_path,
            native_thread_id: native_thread_id.into(),
            parent_thread_id,
            activity_epoch: None,
        }
    }

    /// Restrict live qualification to growth beyond this owner's captured
    /// baseline. The caller includes runtime and native-file incarnation in
    /// `epoch`, and samples the size on epoch entry before reading old bytes.
    pub fn with_activity_epoch(
        mut self,
        epoch: String,
        baseline: LifecycleActivityBaseline,
    ) -> Self {
        self.activity_epoch = Some((epoch, baseline));
        self
    }
}

/// Immutable native identity and size sampled before reading an owner's epoch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LifecycleActivityBaseline {
    native_identity: NativeFileIdentity,
    size: u64,
}

impl LifecycleActivityBaseline {
    /// Sample one open handle's identity and size without reading rollout bytes.
    pub fn capture(path: &std::path::Path) -> io::Result<Self> {
        let file = File::open(path)?;
        Ok(Self {
            native_identity: native_file_identity(&file)?,
            size: file.metadata()?.len(),
        })
    }

    /// Check that the path still resolves to the baseline's native file.
    pub fn matches_file(&self, path: &std::path::Path) -> bool {
        File::open(path)
            .and_then(|file| native_file_identity(&file))
            .is_ok_and(|identity| identity == self.native_identity)
    }
}

/// Identity of the current in-memory generation for one validated rollout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LifecycleSourceIdentity {
    pub canonical_path: PathBuf,
    pub native_thread_id: String,
    pub generation: u64,
}

/// Why the observer is publishing `Unknown` for this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePendingReason {
    SourceUnavailable,
    NativeIdentityUnavailable,
    SourceCapacity,
    BootstrapIncomplete,
    PartialLine,
    OversizeRecord,
    PartialCapacity,
    NoLifecycleMarker,
    OpenTurnNotQualified,
}

/// A source failed header identity validation and must not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleRejection {
    InvalidExpectedIdentity,
    InvalidHeader,
    ThreadIdentityMismatch,
    ParentIdentityMismatch,
    HeaderTooLarge,
}

/// Explicit current native-control evidence supplied by the caller.
///
/// `Unobserved` includes an untracked control ID or native `NOT_FOUND`; it does
/// not imply that the provider child has exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentNativePresence {
    Unobserved,
    Present,
}

/// Lifecycle result safe for the caller to publish after ownership validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecyclePublication {
    Pending {
        source: LifecycleSourceIdentity,
        reason: LifecyclePendingReason,
    },
    Qualified {
        snapshot: CodexLifecycleSnapshot,
    },
    Rejected {
        source: LifecycleSourceIdentity,
        reason: LifecycleRejection,
    },
}

/// Folded provider lifecycle and original provider timestamps for one rollout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexLifecycleSnapshot {
    pub source: LifecycleSourceIdentity,
    pub state: TemporaryWorkerState,
    pub outcome: Option<String>,
    pub requested_at: Option<String>,
    pub turn_id: Option<String>,
    pub turn_started_at: Option<String>,
    pub terminal_at: Option<String>,
}

/// Shared byte allowance for all rollout files visited in one reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleReadBudget {
    remaining: u64,
    consumed: u64,
}

impl LifecycleReadBudget {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            remaining: max_bytes,
            consumed: 0,
        }
    }

    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    fn record_read(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        self.remaining -= bytes;
        self.consumed += bytes;
    }
}

/// Single mutable owner for watcher and periodic lifecycle observations.
///
/// The checkpoint cache and all incomplete-line payload together are bounded.
/// A stage holds an exclusive borrow until its publication is committed or
/// discarded, preventing older scans from racing later turns.
pub struct LifecycleObserver {
    checkpoints: HashMap<SourceKey, Checkpoint>,
    uncommitted: Option<UncommittedStage>,
    cached_partial_bytes: usize,
    parser_version: u32,
    limits: LifecycleLimits,
}

impl Default for LifecycleObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleObserver {
    pub fn new() -> Self {
        Self {
            checkpoints: HashMap::new(),
            uncommitted: None,
            cached_partial_bytes: 0,
            parser_version: CODEX_LIFECYCLE_PARSER_VERSION,
            limits: LifecycleLimits::default(),
        }
    }

    /// Forget paths no longer present in the validated source topology.
    pub fn retain_canonical_paths(&mut self, live_paths: &[PathBuf]) {
        let stale: Vec<SourceKey> = self
            .checkpoints
            .keys()
            .filter(|key| !live_paths.iter().any(|path| path == &key.canonical_path))
            .cloned()
            .collect();
        for key in stale {
            self.take_checkpoint(&key);
        }
        if self.uncommitted.as_ref().is_some_and(|pending| {
            !live_paths
                .iter()
                .any(|path| path == &pending.key.canonical_path)
        }) {
            if let Some(checkpoint) = self
                .uncommitted
                .take()
                .and_then(|pending| pending.checkpoint)
            {
                self.cached_partial_bytes = self
                    .cached_partial_bytes
                    .saturating_sub(checkpoint.partial_line.len());
            }
        }
    }

    /// Reopen a publication whose DB write failed or was interrupted.
    ///
    /// The staged bytes and fold are retained in memory, so retrying publication
    /// never rescans the rollout. Do not observe another source until this stage
    /// commits or is retried successfully.
    pub fn retry_pending(&mut self) -> Option<LifecycleStage<'_>> {
        let pending = self.uncommitted.take()?;
        if let Some(checkpoint) = &pending.checkpoint {
            self.cached_partial_bytes = self
                .cached_partial_bytes
                .saturating_sub(checkpoint.partial_line.len());
        }
        Some(LifecycleStage::new(
            self,
            pending.key,
            pending.checkpoint,
            pending.publication,
        ))
    }

    /// Read one source within the pass-wide budget and stage its publication.
    ///
    /// Commit only after the caller's worker-row publication succeeds. An
    /// incomplete or unqualified scan publishes `Pending` so the caller can
    /// explicitly clear stale terminal fields while the full fold stays private.
    pub fn observe<'a>(
        &'a mut self,
        source: &ValidatedCodexSource,
        budget: &mut LifecycleReadBudget,
        native_presence: CurrentNativePresence,
    ) -> Result<LifecycleStage<'a>, LifecycleObservationError> {
        if self.uncommitted.is_some() {
            return Err(LifecycleObservationError::UncommittedPublication);
        }
        Ok(self.observe_ready(source, budget, native_presence))
    }

    fn observe_ready<'a>(
        &'a mut self,
        source: &ValidatedCodexSource,
        budget: &mut LifecycleReadBudget,
        native_presence: CurrentNativePresence,
    ) -> LifecycleStage<'a> {
        let key = SourceKey {
            canonical_path: std::fs::canonicalize(&source.canonical_path)
                .unwrap_or_else(|_| source.canonical_path.clone()),
            native_thread_id: source.native_thread_id.clone(),
        };
        let fallback_generation = self.remove_other_threads_at_path(&key);
        let previous = self.take_checkpoint(&key);
        let old_generation = previous
            .as_ref()
            .map(|checkpoint| checkpoint.generation)
            .into_iter()
            .chain(fallback_generation)
            .max()
            .unwrap_or(0);

        if source.native_thread_id.trim().is_empty() {
            return self.rejected_stage(
                key,
                old_generation.saturating_add(1),
                None,
                LifecycleRejection::InvalidExpectedIdentity,
            );
        }

        if previous.is_none() && self.checkpoints.len() >= self.limits.max_tracked_sources {
            return self.pending_stage(
                key,
                old_generation.saturating_add(1).max(1),
                None,
                LifecyclePendingReason::SourceCapacity,
            );
        }

        let mut file = match File::open(&key.canonical_path) {
            Ok(file) => file,
            Err(_) => {
                let generation = previous
                    .as_ref()
                    .map(|checkpoint| checkpoint.generation)
                    .unwrap_or_else(|| old_generation.saturating_add(1).max(1));
                return self.pending_stage(
                    key,
                    generation,
                    previous,
                    LifecyclePendingReason::SourceUnavailable,
                );
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                let generation = previous
                    .as_ref()
                    .map(|checkpoint| checkpoint.generation)
                    .unwrap_or_else(|| old_generation.saturating_add(1).max(1));
                return self.pending_stage(
                    key,
                    generation,
                    previous,
                    LifecyclePendingReason::SourceUnavailable,
                );
            }
        };
        let file_len = metadata.len();
        let modified_at = match metadata.modified() {
            Ok(modified_at) => modified_at,
            Err(_) => {
                let generation = previous
                    .as_ref()
                    .map(|checkpoint| checkpoint.generation)
                    .unwrap_or_else(|| old_generation.saturating_add(1).max(1));
                return self.pending_stage(
                    key,
                    generation,
                    previous,
                    LifecyclePendingReason::SourceUnavailable,
                );
            }
        };
        let native_file_identity = match native_file_identity(&file) {
            Ok(identity) => identity,
            Err(_) => {
                let generation = previous
                    .as_ref()
                    .map(|checkpoint| checkpoint.generation)
                    .unwrap_or_else(|| old_generation.saturating_add(1).max(1));
                return self.pending_stage(
                    key,
                    generation,
                    previous,
                    LifecyclePendingReason::NativeIdentityUnavailable,
                );
            }
        };

        let reset = previous.as_ref().is_none_or(|checkpoint| {
            checkpoint.native_file_identity != native_file_identity
                || file_len < checkpoint.observed_size
                || file_len < checkpoint.read_offset
                || file_len < checkpoint.complete_offset
                || (file_len == checkpoint.observed_size && modified_at != checkpoint.modified_at)
                || checkpoint.parser_version != self.parser_version
        });
        let grew_since_observation = !reset
            && previous
                .as_ref()
                .is_some_and(|checkpoint| file_len > checkpoint.observed_size);
        let generation = if reset {
            old_generation.saturating_add(1).max(1)
        } else {
            previous
                .as_ref()
                .map(|checkpoint| checkpoint.generation)
                .unwrap_or(1)
        };
        let mut checkpoint = if reset {
            Checkpoint::new(
                native_file_identity.clone(),
                modified_at,
                generation,
                self.parser_version,
                file_len,
            )
        } else {
            previous.expect("non-reset lifecycle checkpoint exists")
        };
        let activity_epoch = source
            .activity_epoch
            .as_ref()
            .map(|(epoch, _)| epoch.as_str());
        let activity_baseline = source.activity_epoch.as_ref().map(|(_, baseline)| baseline);
        if checkpoint.activity_epoch.as_deref() != activity_epoch
            || checkpoint.activity_baseline_identity.as_ref()
                != activity_baseline.map(|baseline| &baseline.native_identity)
            || activity_baseline
                .is_some_and(|baseline| baseline.size != checkpoint.activity_baseline)
        {
            checkpoint.activity_epoch = activity_epoch.map(str::to_owned);
            checkpoint.positive_activity = false;
            checkpoint.activity_baseline = source
                .activity_epoch
                .as_ref()
                .map_or(file_len, |(_, baseline)| baseline.size);
            checkpoint.activity_baseline_identity =
                activity_baseline.map(|baseline| baseline.native_identity.clone());
        }
        let baseline_identity_matches = source
            .activity_epoch
            .as_ref()
            .is_none_or(|(_, baseline)| baseline.native_identity == native_file_identity);
        if !baseline_identity_matches {
            checkpoint.positive_activity = false;
        }
        checkpoint.positive_activity |=
            if source.activity_epoch.is_some() && baseline_identity_matches {
                file_len > checkpoint.activity_baseline
            } else if source.activity_epoch.is_none() {
                grew_since_observation
            } else {
                false
            };
        checkpoint.observed_size = file_len;

        if checkpoint.header_rejected.is_some() {
            let reason = checkpoint
                .header_rejected
                .clone()
                .unwrap_or(LifecycleRejection::InvalidHeader);
            return self.rejected_stage(key, checkpoint.generation, Some(checkpoint), reason);
        }

        if checkpoint.poisoned.is_none()
            && checkpoint.read_offset < file_len
            && budget.remaining() > 0
        {
            if file.seek(SeekFrom::Start(checkpoint.read_offset)).is_err() {
                return self.pending_stage(
                    key,
                    checkpoint.generation,
                    Some(checkpoint),
                    LifecyclePendingReason::SourceUnavailable,
                );
            }
            let mut buffer = [0_u8; IO_CHUNK_BYTES];
            while checkpoint.read_offset < file_len && budget.remaining() > 0 {
                let allowed = (file_len - checkpoint.read_offset)
                    .min(budget.remaining())
                    .min(buffer.len() as u64) as usize;
                match file.read(&mut buffer[..allowed]) {
                    Ok(0) => break,
                    Ok(read) => {
                        budget.record_read(read);
                        let mut stop = false;
                        for byte in &buffer[..read] {
                            checkpoint.read_offset += 1;
                            if *byte == b'\n' {
                                let line = std::mem::take(&mut checkpoint.partial_line);
                                if checkpoint.header_validated {
                                    fold_event_line(&mut checkpoint.fold, &line);
                                } else if let Err(reason) =
                                    validate_header(&mut checkpoint, source, &line)
                                {
                                    checkpoint.header_rejected = Some(reason);
                                    stop = true;
                                    break;
                                }
                                checkpoint.complete_offset = checkpoint.read_offset;
                            } else {
                                let line_limit = if checkpoint.header_validated {
                                    self.limits.max_record_bytes
                                } else {
                                    MAX_HEADER_BYTES
                                };
                                let total_partial = self
                                    .cached_partial_bytes
                                    .saturating_add(checkpoint.partial_line.len());
                                if checkpoint.partial_line.len() >= line_limit {
                                    checkpoint.partial_line = Vec::new();
                                    if checkpoint.header_validated {
                                        checkpoint.poisoned =
                                            Some(LifecyclePendingReason::OversizeRecord);
                                    } else {
                                        checkpoint.header_rejected =
                                            Some(LifecycleRejection::HeaderTooLarge);
                                    }
                                    stop = true;
                                    break;
                                }
                                if total_partial >= self.limits.max_cached_partial_bytes {
                                    checkpoint.poisoned =
                                        Some(LifecyclePendingReason::PartialCapacity);
                                    checkpoint.partial_line = Vec::new();
                                    stop = true;
                                    break;
                                }
                                checkpoint.partial_line.push(*byte);
                            }
                        }
                        if stop {
                            break;
                        }
                    }
                    Err(_) => {
                        return self.pending_stage(
                            key,
                            checkpoint.generation,
                            Some(checkpoint),
                            LifecyclePendingReason::SourceUnavailable,
                        );
                    }
                }
            }
        }

        let end_metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                return self.pending_stage(
                    key,
                    checkpoint.generation,
                    Some(checkpoint),
                    LifecyclePendingReason::SourceUnavailable,
                );
            }
        };
        let end_len = end_metadata.len();
        let end_modified_at = match end_metadata.modified() {
            Ok(modified_at) => modified_at,
            Err(_) => {
                return self.pending_stage(
                    key,
                    checkpoint.generation,
                    Some(checkpoint),
                    LifecyclePendingReason::SourceUnavailable,
                );
            }
        };
        if end_len < checkpoint.read_offset
            || end_len < checkpoint.observed_size
            || (end_len == file_len && end_modified_at != modified_at)
        {
            let next_generation = checkpoint.generation.saturating_add(1);
            checkpoint = Checkpoint::new(
                native_file_identity,
                end_modified_at,
                next_generation,
                self.parser_version,
                end_len,
            );
        } else {
            if end_len > file_len
                && baseline_identity_matches
                && (source.activity_epoch.is_none() || end_len > checkpoint.activity_baseline)
            {
                checkpoint.positive_activity = true;
            }
            checkpoint.observed_size = end_len;
            checkpoint.modified_at = end_modified_at;
        }

        let identity = self.identity(&key, checkpoint.generation);
        let publication = if let Some(reason) = checkpoint.header_rejected.clone() {
            LifecyclePublication::Rejected {
                source: identity,
                reason,
            }
        } else if let Some(reason) = checkpoint.poisoned {
            LifecyclePublication::Pending {
                source: identity,
                reason,
            }
        } else if !checkpoint.header_validated || checkpoint.read_offset < checkpoint.observed_size
        {
            LifecyclePublication::Pending {
                source: identity,
                reason: LifecyclePendingReason::BootstrapIncomplete,
            }
        } else if !checkpoint.partial_line.is_empty() {
            LifecyclePublication::Pending {
                source: identity,
                reason: LifecyclePendingReason::PartialLine,
            }
        } else {
            match checkpoint.fold.state {
                Some(state) if state.is_terminal() => LifecyclePublication::Qualified {
                    snapshot: checkpoint.snapshot(identity),
                },
                Some(TemporaryWorkerState::Running)
                    if checkpoint.positive_activity
                        || native_presence == CurrentNativePresence::Present =>
                {
                    LifecyclePublication::Qualified {
                        snapshot: checkpoint.snapshot(identity),
                    }
                }
                Some(TemporaryWorkerState::Running) => LifecyclePublication::Pending {
                    source: identity,
                    reason: LifecyclePendingReason::OpenTurnNotQualified,
                },
                _ => LifecyclePublication::Pending {
                    source: identity,
                    reason: LifecyclePendingReason::NoLifecycleMarker,
                },
            }
        };

        LifecycleStage::new(self, key, Some(checkpoint), publication)
    }

    fn identity(&self, key: &SourceKey, generation: u64) -> LifecycleSourceIdentity {
        LifecycleSourceIdentity {
            canonical_path: key.canonical_path.clone(),
            native_thread_id: key.native_thread_id.clone(),
            generation,
        }
    }

    fn pending_stage<'a>(
        &'a mut self,
        key: SourceKey,
        generation: u64,
        checkpoint: Option<Checkpoint>,
        reason: LifecyclePendingReason,
    ) -> LifecycleStage<'a> {
        let source = self.identity(&key, generation);
        LifecycleStage::new(
            self,
            key,
            checkpoint,
            LifecyclePublication::Pending { source, reason },
        )
    }

    fn rejected_stage<'a>(
        &'a mut self,
        key: SourceKey,
        generation: u64,
        checkpoint: Option<Checkpoint>,
        reason: LifecycleRejection,
    ) -> LifecycleStage<'a> {
        let source = self.identity(&key, generation);
        LifecycleStage::new(
            self,
            key,
            checkpoint,
            LifecyclePublication::Rejected { source, reason },
        )
    }

    fn take_checkpoint(&mut self, key: &SourceKey) -> Option<Checkpoint> {
        let checkpoint = self.checkpoints.remove(key)?;
        self.cached_partial_bytes = self
            .cached_partial_bytes
            .saturating_sub(checkpoint.partial_line.len());
        Some(checkpoint)
    }

    fn remove_other_threads_at_path(&mut self, key: &SourceKey) -> Option<u64> {
        let stale: Vec<SourceKey> = self
            .checkpoints
            .keys()
            .filter(|existing| {
                existing.canonical_path == key.canonical_path
                    && existing.native_thread_id != key.native_thread_id
            })
            .cloned()
            .collect();
        stale
            .into_iter()
            .filter_map(|stale_key| {
                self.take_checkpoint(&stale_key)
                    .map(|checkpoint| checkpoint.generation)
            })
            .max()
    }
}

/// Candidate publication that holds the observer's exclusive writer borrow.
pub struct LifecycleStage<'a> {
    observer: &'a mut LifecycleObserver,
    key: SourceKey,
    checkpoint: Option<Checkpoint>,
    publication: LifecyclePublication,
    committed: bool,
}

impl<'a> LifecycleStage<'a> {
    fn new(
        observer: &'a mut LifecycleObserver,
        key: SourceKey,
        checkpoint: Option<Checkpoint>,
        publication: LifecyclePublication,
    ) -> Self {
        Self {
            observer,
            key,
            checkpoint,
            publication,
            committed: false,
        }
    }

    pub fn publication(&self) -> &LifecyclePublication {
        &self.publication
    }

    /// Commit the cache only after a Pending or Qualified DB publication succeeds.
    pub fn commit(mut self) -> Result<(), LifecycleCommitError> {
        if matches!(&self.publication, LifecyclePublication::Rejected { .. }) {
            return Err(LifecycleCommitError::RejectedSource);
        }
        self.install_checkpoint();
        self.committed = true;
        Ok(())
    }

    /// Cache a stable header rejection without publishing a worker row.
    pub fn commit_rejection(mut self) -> Result<(), LifecycleCommitError> {
        if !matches!(&self.publication, LifecyclePublication::Rejected { .. }) {
            return Err(LifecycleCommitError::NotRejected);
        }
        self.install_checkpoint();
        self.committed = true;
        Ok(())
    }

    fn install_checkpoint(&mut self) {
        if let Some(checkpoint) = self.checkpoint.take() {
            self.observer.cached_partial_bytes = self
                .observer
                .cached_partial_bytes
                .saturating_add(checkpoint.partial_line.len());
            self.observer
                .checkpoints
                .insert(self.key.clone(), checkpoint);
        }
    }
}

impl Drop for LifecycleStage<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(checkpoint) = &self.checkpoint {
                self.observer.cached_partial_bytes = self
                    .observer
                    .cached_partial_bytes
                    .saturating_add(checkpoint.partial_line.len());
            }
            self.observer.uncommitted = Some(UncommittedStage {
                key: self.key.clone(),
                checkpoint: self.checkpoint.take(),
                publication: self.publication.clone(),
            });
        }
    }
}

struct UncommittedStage {
    key: SourceKey,
    checkpoint: Option<Checkpoint>,
    publication: LifecyclePublication,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleObservationError {
    UncommittedPublication,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCommitError {
    RejectedSource,
    NotRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceKey {
    canonical_path: PathBuf,
    native_thread_id: String,
}

#[derive(Debug, Clone)]
struct Checkpoint {
    native_file_identity: NativeFileIdentity,
    modified_at: SystemTime,
    generation: u64,
    parser_version: u32,
    observed_size: u64,
    read_offset: u64,
    complete_offset: u64,
    partial_line: Vec<u8>,
    header_validated: bool,
    header_rejected: Option<LifecycleRejection>,
    poisoned: Option<LifecyclePendingReason>,
    positive_activity: bool,
    activity_epoch: Option<String>,
    activity_baseline: u64,
    activity_baseline_identity: Option<NativeFileIdentity>,
    fold: LifecycleFold,
}

impl Checkpoint {
    fn new(
        native_file_identity: NativeFileIdentity,
        modified_at: SystemTime,
        generation: u64,
        parser_version: u32,
        observed_size: u64,
    ) -> Self {
        Self {
            native_file_identity,
            modified_at,
            generation,
            parser_version,
            observed_size,
            read_offset: 0,
            complete_offset: 0,
            partial_line: Vec::new(),
            header_validated: false,
            header_rejected: None,
            poisoned: None,
            positive_activity: false,
            activity_epoch: None,
            activity_baseline: observed_size,
            activity_baseline_identity: None,
            fold: LifecycleFold::default(),
        }
    }

    fn snapshot(&self, source: LifecycleSourceIdentity) -> CodexLifecycleSnapshot {
        CodexLifecycleSnapshot {
            source,
            state: self.fold.state.unwrap_or(TemporaryWorkerState::Unknown),
            outcome: self.fold.outcome.clone(),
            requested_at: self.fold.requested_at.clone(),
            turn_id: self.fold.turn_id.clone(),
            turn_started_at: self.fold.turn_started_at.clone(),
            terminal_at: self.fold.terminal_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct LifecycleFold {
    state: Option<TemporaryWorkerState>,
    turn_id: Option<String>,
    turn_started_at: Option<String>,
    outcome: Option<String>,
    terminal_at: Option<String>,
    requested_at: Option<String>,
    active_turn_id: Option<String>,
    legacy_turn_active: bool,
    saw_task_started: bool,
    last_terminal_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeFileIdentity {
    platform: &'static str,
    primary: u64,
    secondary: u64,
}

#[derive(Debug, Clone, Copy)]
struct LifecycleLimits {
    max_record_bytes: usize,
    max_cached_partial_bytes: usize,
    max_tracked_sources: usize,
}

impl Default for LifecycleLimits {
    fn default() -> Self {
        Self {
            max_record_bytes: MAX_RECORD_BYTES,
            max_cached_partial_bytes: MAX_CACHED_PARTIAL_BYTES,
            max_tracked_sources: MAX_TRACKED_SOURCES,
        }
    }
}

fn validate_header(
    checkpoint: &mut Checkpoint,
    source: &ValidatedCodexSource,
    line: &[u8],
) -> Result<(), LifecycleRejection> {
    let value: Value =
        serde_json::from_slice(line).map_err(|_| LifecycleRejection::InvalidHeader)?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err(LifecycleRejection::InvalidHeader);
    }
    let payload = value
        .get("payload")
        .ok_or(LifecycleRejection::InvalidHeader)?;
    let thread_id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(LifecycleRejection::InvalidHeader)?;
    if thread_id != source.native_thread_id {
        return Err(LifecycleRejection::ThreadIdentityMismatch);
    }
    let parent_thread_id = payload
        .get("source")
        .and_then(|value| value.get("subagent"))
        .and_then(|value| value.get("thread_spawn"))
        .and_then(|value| value.get("parent_thread_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if parent_thread_id != source.parent_thread_id.as_deref() {
        return Err(LifecycleRejection::ParentIdentityMismatch);
    }
    checkpoint.fold.requested_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    checkpoint.header_validated = true;
    Ok(())
}

fn fold_event_line(fold: &mut LifecycleFold, line: &[u8]) {
    let Ok(event) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    let event_type = event.get("type").and_then(Value::as_str);
    let payload = event.get("payload");
    let payload_type = payload
        .and_then(|payload| payload.get("type"))
        .and_then(Value::as_str);
    let turn_id = payload
        .and_then(|payload| payload.get("turn_id"))
        .and_then(Value::as_str)
        .or_else(|| event.get("turn_id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let timestamp = event
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);

    match (event_type, payload_type) {
        (Some("event_msg"), Some("task_started")) => {
            if turn_id
                .as_deref()
                .is_some_and(|turn| fold.last_terminal_turn_id.as_deref() == Some(turn))
                && fold.active_turn_id.is_none()
            {
                return;
            }
            fold.state = Some(TemporaryWorkerState::Running);
            fold.turn_id = turn_id.clone();
            fold.active_turn_id = turn_id;
            fold.legacy_turn_active = false;
            fold.saw_task_started = true;
            fold.turn_started_at = timestamp;
            fold.outcome = None;
            fold.terminal_at = None;
            fold.last_terminal_turn_id = None;
        }
        (Some("event_msg"), Some("user_message" | "user_message_event")) => {
            // Codex mirrors an identified turn's prompt without a turn ID.
            // Its task_started identity must survive until the keyed terminal.
            if fold.active_turn_id.is_some() {
                return;
            }
            fold.state = Some(TemporaryWorkerState::Running);
            fold.turn_id = None;
            fold.active_turn_id = None;
            fold.legacy_turn_active = true;
            fold.turn_started_at = timestamp;
            fold.outcome = None;
            fold.terminal_at = None;
            fold.last_terminal_turn_id = None;
        }
        (Some("event_msg"), Some("task_complete")) | (Some("turn.completed"), _) => {
            apply_terminal(
                fold,
                turn_id.as_deref(),
                timestamp.as_deref(),
                TemporaryWorkerState::Succeeded,
                "completed",
            );
        }
        (Some("event_msg"), Some("turn_aborted")) => {
            apply_terminal(
                fold,
                turn_id.as_deref(),
                timestamp.as_deref(),
                TemporaryWorkerState::Cancelled,
                "aborted",
            );
        }
        _ => {}
    }
}

fn apply_terminal(
    fold: &mut LifecycleFold,
    turn_id: Option<&str>,
    timestamp: Option<&str>,
    state: TemporaryWorkerState,
    outcome: &str,
) {
    let matches_open_turn = match fold.active_turn_id.as_deref() {
        Some(active) => turn_id == Some(active),
        None if fold.legacy_turn_active => turn_id.is_none(),
        None if fold.saw_task_started => false,
        None => true,
    };
    if !matches_open_turn {
        return;
    }
    if fold.active_turn_id.is_none()
        && !fold.legacy_turn_active
        && fold.last_terminal_turn_id.as_deref() == turn_id
        && turn_id.is_some()
    {
        return;
    }
    fold.state = Some(state);
    fold.turn_id = turn_id.map(str::to_string).or_else(|| fold.turn_id.clone());
    fold.active_turn_id = None;
    fold.legacy_turn_active = false;
    fold.last_terminal_turn_id = turn_id.map(str::to_string);
    fold.outcome = Some(outcome.to_string());
    fold.terminal_at = timestamp.map(str::to_string);
}

fn native_file_identity(file: &File) -> io::Result<NativeFileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        return Ok(NativeFileIdentity {
            platform: "unix",
            primary: metadata.dev(),
            secondary: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle as _;

        #[repr(C)]
        #[allow(non_snake_case)]
        struct FileTime {
            dwLowDateTime: u32,
            dwHighDateTime: u32,
        }
        #[repr(C)]
        #[allow(non_snake_case)]
        struct ByHandleFileInformation {
            dwFileAttributes: u32,
            ftCreationTime: FileTime,
            ftLastAccessTime: FileTime,
            ftLastWriteTime: FileTime,
            dwVolumeSerialNumber: u32,
            nFileSizeHigh: u32,
            nFileSizeLow: u32,
            nNumberOfLinks: u32,
            nFileIndexHigh: u32,
            nFileIndexLow: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                file: *mut c_void,
                information: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
        // SAFETY: `file` is an open OS handle and Windows initializes the full
        // output structure when the call reports success.
        let succeeded =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful call above initialized `information`.
        let information = unsafe { information.assume_init() };
        return Ok(NativeFileIdentity {
            platform: "windows",
            primary: u64::from(information.dwVolumeSerialNumber),
            secondary: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        });
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Codex rollout native identity is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    impl LifecycleReadBudget {
        pub fn for_pass() -> Self {
            Self::new(CODEX_LIFECYCLE_BYTES_PER_PASS)
        }
    }

    impl LifecycleObserver {
        /// Change the parser generation. Each existing source resets on next read.
        pub fn set_parser_version(&mut self, parser_version: u32) {
            self.parser_version = parser_version;
        }

        pub fn cached_partial_bytes(&self) -> usize {
            self.cached_partial_bytes
        }
    }

    #[test]
    fn retaining_paths_preserves_pending_retry_and_removes_only_obsolete_source() {
        let directory = tempfile::tempdir().unwrap();
        let a_path = directory.path().join("a.jsonl");
        let b_path = directory.path().join("b.jsonl");
        fs::write(&a_path, format!("{}{{\"type\":", header("A", "parent"))).unwrap();
        fs::write(
            &b_path,
            format!(
                "{}{}{}",
                header("B", "parent"),
                event("task_started", "T", "2026-10-04T21:10:00Z"),
                event("task_complete", "T", "2026-10-04T21:11:00Z")
            ),
        )
        .unwrap();
        let a = source(&a_path, "A");
        let b = source(&b_path, "B");
        let mut observer = LifecycleObserver::new();
        let b_publication = stage(
            &mut observer,
            &b,
            &mut LifecycleReadBudget::for_pass(),
            CurrentNativePresence::Unobserved,
        );
        let pending = observer
            .observe(
                &a,
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved,
            )
            .unwrap();
        let identity = match pending.publication() {
            LifecyclePublication::Pending { source, .. } => source.clone(),
            other => panic!("expected pending partial line: {other:?}"),
        };
        drop(pending);
        assert!(observer.cached_partial_bytes() > 0);
        let retained_bytes = observer.cached_partial_bytes();
        observer.retain_canonical_paths(&[a.canonical_path.clone(), b.canonical_path.clone()]);
        assert_eq!(observer.cached_partial_bytes(), retained_bytes);
        let retry = observer.retry_pending().unwrap();
        assert!(
            matches!(retry.publication(), LifecyclePublication::Pending { source, .. } if source == &identity)
        );
        drop(retry);
        assert_eq!(observer.cached_partial_bytes(), retained_bytes);
        observer.retain_canonical_paths(std::slice::from_ref(&b.canonical_path));
        assert!(observer.retry_pending().is_none());
        assert_eq!(observer.cached_partial_bytes(), 0);
        let mut unchanged_budget = LifecycleReadBudget::new(0);
        let unchanged = stage(
            &mut observer,
            &b,
            &mut unchanged_budget,
            CurrentNativePresence::Unobserved,
        );
        assert_eq!(unchanged, b_publication);
        assert_eq!(unchanged_budget.consumed(), 0);
    }

    fn header(thread: &str, parent: &str) -> String {
        format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-10-04T21:00:00Z\",\"payload\":{{\"id\":\"{thread}\",\"source\":{{\"subagent\":{{\"thread_spawn\":{{\"parent_thread_id\":\"{parent}\"}}}}}}}}}}\n"
        )
    }

    #[test]
    fn identified_turn_survives_real_user_message_before_complete_or_abort() {
        let mirror = include_str!("../../providers/fixtures/codex-real-delivery-mirror.jsonl");
        let lines: Vec<_> = mirror.lines().take(4).collect();
        let started: Value = serde_json::from_str(lines[0]).unwrap();
        let turn = started["payload"]["turn_id"].as_str().unwrap();
        for (terminal, expected) in [
            ("task_complete", TemporaryWorkerState::Succeeded),
            ("turn_aborted", TemporaryWorkerState::Cancelled),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("child.jsonl");
            fs::write(
                &path,
                format!(
                    "{}{}\n{}\n{}",
                    header("child", "parent"),
                    lines[0],
                    lines[3],
                    event(terminal, turn, "2026-09-14T21:50:00Z")
                ),
            )
            .unwrap();
            let mut observer = LifecycleObserver::new();
            let publication = stage(
                &mut observer,
                &source(&path, "child"),
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved,
            );
            let LifecyclePublication::Qualified { snapshot } = publication else {
                panic!("expected matching terminal");
            };
            assert_eq!(snapshot.state, expected);
            assert_eq!(snapshot.turn_id.as_deref(), Some(turn));
            assert_eq!(
                snapshot.turn_started_at.as_deref(),
                Some("2026-09-14T21:49:06.819Z")
            );
        }
    }

    #[test]
    fn identified_turn_survives_user_message_and_terminal_split_across_appends() {
        for (terminal, expected) in [
            ("task_complete", TemporaryWorkerState::Succeeded),
            ("turn_aborted", TemporaryWorkerState::Cancelled),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("child.jsonl");
            fs::write(
                &path,
                format!(
                    "{}{}",
                    header("child", "parent"),
                    event("task_started", "T", "2026-10-05T02:00:00Z")
                ),
            )
            .unwrap();
            let source = source(&path, "child");
            let mut observer = LifecycleObserver::new();
            assert!(matches!(
                stage(
                    &mut observer,
                    &source,
                    &mut LifecycleReadBudget::for_pass(),
                    CurrentNativePresence::Unobserved
                ),
                LifecyclePublication::Pending { .. }
            ));
            let terminal = event(terminal, "T", "2026-10-05T02:02:00Z");
            let split = terminal.len() / 2;
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(b"{\"timestamp\":\"2026-10-05T02:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"prompt\"}}\n").unwrap();
            file.write_all(&terminal.as_bytes()[..split]).unwrap();
            file.flush().unwrap();
            assert!(matches!(
                stage(
                    &mut observer,
                    &source,
                    &mut LifecycleReadBudget::for_pass(),
                    CurrentNativePresence::Unobserved
                ),
                LifecyclePublication::Pending {
                    reason: LifecyclePendingReason::PartialLine,
                    ..
                }
            ));
            file.write_all(&terminal.as_bytes()[split..]).unwrap();
            file.flush().unwrap();
            let publication = stage(
                &mut observer,
                &source,
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved,
            );
            let LifecyclePublication::Qualified { snapshot } = publication else {
                panic!("expected split terminal");
            };
            assert_eq!(snapshot.state, expected);
            assert_eq!(snapshot.turn_id.as_deref(), Some("T"));
        }
    }

    #[test]
    fn legacy_unkeyed_user_turn_still_accepts_unkeyed_complete_and_abort() {
        for (terminal, expected) in [
            ("task_complete", TemporaryWorkerState::Succeeded),
            ("turn_aborted", TemporaryWorkerState::Cancelled),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("child.jsonl");
            let user = serde_json::json!({"type":"event_msg", "payload":{"type":"user_message"}, "timestamp":"2026-10-05T02:00:00Z"});
            let terminal = serde_json::json!({"type":"event_msg", "payload":{"type":terminal}, "timestamp":"2026-10-05T02:01:00Z"});
            fs::write(
                &path,
                format!("{}{user}\n{terminal}\n", header("child", "parent")),
            )
            .unwrap();
            let mut observer = LifecycleObserver::new();
            let publication = stage(
                &mut observer,
                &source(&path, "child"),
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved,
            );
            let LifecyclePublication::Qualified { snapshot } = publication else {
                panic!("expected legacy terminal");
            };
            assert_eq!(snapshot.state, expected);
            assert!(snapshot.turn_id.is_none());
        }
    }

    #[test]
    fn owner_epoch_change_preserves_fold_but_requires_post_epoch_activity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("child.jsonl");
        fs::write(&path, header("child", "parent")).unwrap();
        let original = source(&path, "child").with_activity_epoch(
            "G".into(),
            LifecycleActivityBaseline::capture(&path).unwrap(),
        );
        let mut observer = LifecycleObserver::new();
        stage(
            &mut observer,
            &original,
            &mut LifecycleReadBudget::for_pass(),
            CurrentNativePresence::Unobserved,
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(event("task_started", "T", "2026-10-05T02:00:00Z").as_bytes())
            .unwrap();
        file.flush().unwrap();
        assert!(
            matches!(stage(&mut observer, &original, &mut LifecycleReadBudget::for_pass(), CurrentNativePresence::Unobserved),
            LifecyclePublication::Qualified { snapshot } if snapshot.state == TemporaryWorkerState::Running)
        );
        let paused = source(&path, "child").with_activity_epoch(
            "paused".into(),
            LifecycleActivityBaseline::capture(&path).unwrap(),
        );
        let mut zero = LifecycleReadBudget::new(0);
        assert!(matches!(
            stage(
                &mut observer,
                &paused,
                &mut zero,
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OpenTurnNotQualified,
                ..
            }
        ));
        assert_eq!(zero.consumed(), 0);
        // These bytes were never parsed in G, but existed before G+1. Neither
        // cached positive evidence nor newly folded old bytes qualify G+1.
        file.write_all(
            event("task_started", "old-before-resume", "2026-10-05T02:01:00Z").as_bytes(),
        )
        .unwrap();
        file.flush().unwrap();
        let resumed = source(&path, "child").with_activity_epoch(
            "G+1".into(),
            LifecycleActivityBaseline::capture(&path).unwrap(),
        );
        let mut small = LifecycleReadBudget::new(8);
        assert!(matches!(
            stage(
                &mut observer,
                &resumed,
                &mut small,
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::BootstrapIncomplete,
                ..
            }
        ));
        assert!(matches!(
            stage(
                &mut observer,
                &resumed,
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OpenTurnNotQualified,
                ..
            }
        ));
        let mut unchanged = LifecycleReadBudget::new(0);
        assert!(matches!(
            stage(
                &mut observer,
                &resumed,
                &mut unchanged,
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OpenTurnNotQualified,
                ..
            }
        ));
        assert_eq!(unchanged.consumed(), 0);
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"current activity\"}}\n").unwrap();
        file.flush().unwrap();
        assert!(
            matches!(stage(&mut observer, &resumed, &mut LifecycleReadBudget::for_pass(), CurrentNativePresence::Unobserved),
            LifecyclePublication::Qualified { snapshot } if snapshot.state == TemporaryWorkerState::Running && snapshot.turn_id.as_deref() == Some("old-before-resume"))
        );
    }

    #[test]
    fn mismatched_native_epoch_baseline_cannot_qualify_cached_or_unread_activity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("child.jsonl");
        fs::write(&path, header("child", "parent")).unwrap();
        let tracked = source(&path, "child").with_activity_epoch(
            "G".into(),
            LifecycleActivityBaseline::capture(&path).unwrap(),
        );
        let mut observer = LifecycleObserver::new();
        stage(
            &mut observer,
            &tracked,
            &mut LifecycleReadBudget::for_pass(),
            CurrentNativePresence::Unobserved,
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(event("task_started", "T", "2026-10-05T02:00:00Z").as_bytes())
            .unwrap();
        file.flush().unwrap();
        assert!(matches!(
            stage(
                &mut observer,
                &tracked,
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Qualified { .. }
        ));
        let other = directory.path().join("other.jsonl");
        fs::write(&other, b"short\n").unwrap();
        let mismatched = source(&path, "child").with_activity_epoch(
            "G+1".into(),
            LifecycleActivityBaseline::capture(&other).unwrap(),
        );
        let mut zero = LifecycleReadBudget::new(0);
        assert!(matches!(
            stage(
                &mut observer,
                &mismatched,
                &mut zero,
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OpenTurnNotQualified,
                ..
            }
        ));
        assert_eq!(zero.consumed(), 0);
        // Correcting the native proof under the same owner label must also
        // correct its size baseline, rather than inheriting the shorter file.
        let corrected = source(&path, "child").with_activity_epoch(
            "G+1".into(),
            LifecycleActivityBaseline::capture(&path).unwrap(),
        );
        assert!(matches!(
            stage(
                &mut observer,
                &corrected,
                &mut LifecycleReadBudget::new(0),
                CurrentNativePresence::Unobserved
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OpenTurnNotQualified,
                ..
            }
        ));
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\"}}\n")
            .unwrap();
        file.flush().unwrap();
        assert!(
            matches!(stage(&mut observer, &corrected, &mut LifecycleReadBudget::for_pass(), CurrentNativePresence::Unobserved),
            LifecyclePublication::Qualified { snapshot } if snapshot.state == TemporaryWorkerState::Running)
        );
    }

    fn event(kind: &str, turn_id: &str, timestamp: &str) -> String {
        format!(
            "{{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\",\"turn_id\":\"{turn_id}\"}}}}\n"
        )
    }

    fn source(path: &Path, thread: &str) -> ValidatedCodexSource {
        ValidatedCodexSource::from_verified_ancestry(
            fs::canonicalize(path).unwrap(),
            thread,
            Some("parent".to_string()),
        )
    }

    fn stage(
        observer: &mut LifecycleObserver,
        source: &ValidatedCodexSource,
        budget: &mut LifecycleReadBudget,
        presence: CurrentNativePresence,
    ) -> LifecyclePublication {
        let staged = observer.observe(source, budget, presence).unwrap();
        let publication = staged.publication().clone();
        if matches!(&publication, LifecyclePublication::Rejected { .. }) {
            staged.commit_rejection().unwrap();
        } else {
            staged.commit().unwrap();
        }
        publication
    }

    #[test]
    fn completed_turn_then_new_start_folds_to_the_new_turn_and_clears_terminal_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!(
                "{}{}{}{}",
                header("child", "parent"),
                event("task_started", "T1", "2026-10-04T21:10:00Z"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
                event("task_started", "T2", "2026-10-04T21:55:00Z"),
            ),
        )
        .unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::for_pass();

        let publication = stage(
            &mut observer,
            &source,
            &mut budget,
            CurrentNativePresence::Unobserved,
        );

        assert!(matches!(
            publication,
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OpenTurnNotQualified,
                ..
            }
        ));
        let checkpoint = observer.checkpoints.values().next().unwrap();
        assert_eq!(checkpoint.fold.state, Some(TemporaryWorkerState::Running));
        assert_eq!(checkpoint.fold.turn_id.as_deref(), Some("T2"));
        assert_eq!(
            checkpoint.fold.turn_started_at.as_deref(),
            Some("2026-10-04T21:55:00Z")
        );
        assert_eq!(checkpoint.fold.outcome, None);
        assert_eq!(checkpoint.fold.terminal_at, None);
    }

    #[test]
    fn mismatched_old_completion_cannot_close_the_newer_turn() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!(
                "{}{}{}{}",
                header("child", "parent"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
                event("task_started", "T2", "2026-10-04T21:55:00Z"),
                event("task_complete", "T1", "2026-10-04T21:56:00Z"),
            ),
        )
        .unwrap();
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::for_pass();
        let publication = stage(
            &mut observer,
            &source(&path, "child"),
            &mut budget,
            CurrentNativePresence::Present,
        );

        let LifecyclePublication::Qualified { snapshot } = publication else {
            panic!("current native presence qualifies the open T2 turn");
        };
        assert_eq!(snapshot.state, TemporaryWorkerState::Running);
        assert_eq!(snapshot.turn_id.as_deref(), Some("T2"));
        assert_eq!(snapshot.outcome, None);
        assert_eq!(snapshot.terminal_at, None);
    }

    #[test]
    fn cold_bootstrap_finds_start_beyond_one_pass_without_reading_the_whole_rollout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large-rollout.jsonl");
        let mut body = header("child", "parent");
        for _ in 0..12_000 {
            body.push_str("{\"type\":\"other\",\"payload\":{}}\n");
        }
        body.push_str(&event("task_started", "T2", "2026-10-04T21:55:00Z"));
        body.push_str(&event("task_complete", "T2", "2026-10-04T21:56:00Z"));
        fs::write(&path, body).unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut first_budget = LifecycleReadBudget::for_pass();

        let first = stage(
            &mut observer,
            &source,
            &mut first_budget,
            CurrentNativePresence::Unobserved,
        );
        assert_eq!(first_budget.consumed(), CODEX_LIFECYCLE_BYTES_PER_PASS);
        assert!(matches!(first, LifecyclePublication::Pending { .. }));

        let mut second_budget = LifecycleReadBudget::for_pass();
        let second = stage(
            &mut observer,
            &source,
            &mut second_budget,
            CurrentNativePresence::Unobserved,
        );
        let LifecyclePublication::Qualified { snapshot } = second else {
            panic!("the later pass reaches and folds the terminal events");
        };
        assert_eq!(snapshot.state, TemporaryWorkerState::Succeeded);
        assert_eq!(snapshot.turn_id.as_deref(), Some("T2"));
        assert!(second_budget.consumed() <= CODEX_LIFECYCLE_BYTES_PER_PASS);
    }

    #[test]
    fn unchanged_caught_up_source_reads_no_body_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!(
                "{}{}",
                header("child", "parent"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
            ),
        )
        .unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut initial = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut initial,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Qualified { .. }
        ));

        let mut unchanged = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut unchanged,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Qualified { .. }
        ));
        assert_eq!(unchanged.consumed(), 0);
    }

    #[test]
    fn byte_budget_is_shared_across_sources_in_one_pass() {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.jsonl");
        let second_path = directory.path().join("second.jsonl");
        let filler = "{\"type\":\"other\",\"payload\":{}}\n".repeat(128);
        fs::write(
            &first_path,
            format!("{}{}", header("first", "parent"), filler),
        )
        .unwrap();
        fs::write(
            &second_path,
            format!(
                "{}{}",
                header("second", "parent"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
            ),
        )
        .unwrap();
        let first_source = source(&first_path, "first");
        let second_source = source(&second_path, "second");
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::new(64);

        let first_stage = observer
            .observe(
                &first_source,
                &mut budget,
                CurrentNativePresence::Unobserved,
            )
            .unwrap();
        assert!(matches!(
            first_stage.publication(),
            LifecyclePublication::Pending { .. }
        ));
        first_stage.commit().unwrap();

        let second_stage = observer
            .observe(
                &second_source,
                &mut budget,
                CurrentNativePresence::Unobserved,
            )
            .unwrap();
        assert!(matches!(
            second_stage.publication(),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::BootstrapIncomplete,
                ..
            }
        ));
        second_stage.commit().unwrap();
        assert_eq!(budget.consumed(), 64);
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn failed_publication_retries_the_staged_checkpoint_without_rescanning() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        let mut body = header("child", "parent");
        body.push_str(&"{\"type\":\"other\",\"payload\":{}}\n".repeat(128));
        body.push_str(&event("task_complete", "T1", "2026-10-04T21:14:00Z"));
        fs::write(&path, body).unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::new(64);

        let failed_stage = observer
            .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
            .unwrap();
        let pending_publication = failed_stage.publication().clone();
        drop(failed_stage);

        assert!(matches!(
            observer.observe(&source, &mut budget, CurrentNativePresence::Unobserved,),
            Err(LifecycleObservationError::UncommittedPublication)
        ));
        let retry = observer.retry_pending().unwrap();
        assert_eq!(retry.publication(), &pending_publication);
        assert_eq!(budget.consumed(), 64);
        retry.commit().unwrap();
    }

    #[test]
    fn partial_line_resumes_at_append_delta_and_new_activity_qualifies_running() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        let start = "{\"timestamp\":\"2026-10-04T21:55:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"T2\"}";
        fs::write(&path, format!("{}{}", header("child", "parent"), start)).unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut first_budget = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut first_budget,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::PartialLine,
                ..
            }
        ));
        let prior_size = fs::metadata(&path).unwrap().len();
        let suffix = "}\n";
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(suffix.as_bytes()).unwrap();

        let mut next_budget = LifecycleReadBudget::for_pass();
        let publication = stage(
            &mut observer,
            &source,
            &mut next_budget,
            CurrentNativePresence::Unobserved,
        );
        let LifecyclePublication::Qualified { snapshot } = publication else {
            panic!("the appended complete start is current activity");
        };
        assert_eq!(
            next_budget.consumed(),
            (fs::metadata(&path).unwrap().len() - prior_size) as u64
        );
        assert_eq!(snapshot.state, TemporaryWorkerState::Running);
        assert_eq!(snapshot.turn_id.as_deref(), Some("T2"));
    }

    #[test]
    fn truncation_and_parser_version_changes_start_a_new_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        let original = format!(
            "{}{}{}",
            header("child", "parent"),
            event("task_started", "T1", "2026-10-04T21:10:00Z"),
            event("task_complete", "T1", "2026-10-04T21:14:00Z"),
        );
        fs::write(&path, original).unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut first_budget = LifecycleReadBudget::for_pass();
        stage(
            &mut observer,
            &source,
            &mut first_budget,
            CurrentNativePresence::Unobserved,
        );
        let first_generation = observer.checkpoints.values().next().unwrap().generation;

        fs::write(&path, header("child", "parent")).unwrap();
        let mut truncate_budget = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut truncate_budget,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::NoLifecycleMarker,
                ..
            }
        ));
        let after_truncate = observer.checkpoints.values().next().unwrap().generation;
        assert_eq!(after_truncate, first_generation + 1);

        observer.set_parser_version(CODEX_LIFECYCLE_PARSER_VERSION + 1);
        let mut parser_budget = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut parser_budget,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::NoLifecycleMarker,
                ..
            }
        ));
        let after_parser_reset = observer.checkpoints.values().next().unwrap().generation;
        assert_eq!(after_parser_reset, after_truncate + 1);
        assert!(parser_budget.consumed() > 0);
    }

    #[test]
    fn replaced_native_file_identity_resets_the_cached_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!(
                "{}{}",
                header("child", "parent"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
            ),
        )
        .unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut initial = LifecycleReadBudget::for_pass();
        stage(
            &mut observer,
            &source,
            &mut initial,
            CurrentNativePresence::Unobserved,
        );
        let prior_generation = observer.checkpoints.values().next().unwrap().generation;
        observer
            .checkpoints
            .values_mut()
            .next()
            .unwrap()
            .native_file_identity
            .secondary ^= 1;

        let mut replacement_budget = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut replacement_budget,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Qualified { .. }
        ));
        assert_eq!(
            observer.checkpoints.values().next().unwrap().generation,
            prior_generation + 1
        );
        assert!(replacement_budget.consumed() > 0);
    }

    #[test]
    fn same_length_rewrite_with_new_modified_time_resets_the_cached_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        let original = format!(
            "{}{}{}",
            header("child", "parent"),
            event("task_started", "turn-0001", "2026-10-04T21:00:00.000Z"),
            event("task_complete", "turn-0001", "2026-10-04T21:00:17.000Z")
        );
        let rewritten = format!(
            "{}{}{}",
            header("child", "parent"),
            event("task_started", "turn-0002", "2026-10-04T21:00:00.000Z"),
            event("task_complete", "turn-0002", "2026-10-04T21:00:17.000Z")
        );
        assert_eq!(original.len(), rewritten.len());

        let first_modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let second_modified = first_modified + Duration::from_secs(1);
        fs::write(&path, original).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(first_modified))
            .unwrap();
        let original_file_identity = native_file_identity(&File::open(&path).unwrap()).unwrap();

        let mut observer = LifecycleObserver::new();
        let source = source(&path, "child");
        let first = match stage(
            &mut observer,
            &source,
            &mut LifecycleReadBudget::for_pass(),
            CurrentNativePresence::Unobserved,
        ) {
            LifecyclePublication::Qualified { snapshot } => snapshot,
            other => panic!("expected first qualified snapshot, got {other:?}"),
        };

        fs::write(&path, rewritten).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(second_modified))
            .unwrap();
        assert_eq!(
            original_file_identity,
            native_file_identity(&File::open(&path).unwrap()).unwrap()
        );
        let second = match stage(
            &mut observer,
            &source,
            &mut LifecycleReadBudget::for_pass(),
            CurrentNativePresence::Unobserved,
        ) {
            LifecyclePublication::Qualified { snapshot } => snapshot,
            other => panic!("expected rewritten qualified snapshot, got {other:?}"),
        };

        assert_eq!(first.turn_id.as_deref(), Some("turn-0001"));
        assert_eq!(second.turn_id.as_deref(), Some("turn-0002"));
        assert!(second.source.generation > first.source.generation);
    }

    #[test]
    fn oversized_record_stays_unknown_without_retaining_its_payload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!("{}{}\n", header("child", "parent"), "x".repeat(1024)),
        )
        .unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        observer.limits.max_record_bytes = 512;
        observer.limits.max_cached_partial_bytes = 1024;
        let mut budget = LifecycleReadBudget::for_pass();

        let publication = stage(
            &mut observer,
            &source,
            &mut budget,
            CurrentNativePresence::Unobserved,
        );
        assert!(matches!(
            publication,
            LifecyclePublication::Pending {
                reason: LifecyclePendingReason::OversizeRecord,
                ..
            }
        ));
        assert_eq!(observer.cached_partial_bytes(), 0);
    }

    #[test]
    fn partial_payload_cap_is_shared_across_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut observer = LifecycleObserver::new();
        observer.limits.max_record_bytes = 512;
        observer.limits.max_cached_partial_bytes = 360;
        let mut budget = LifecycleReadBudget::new(16 * 1024);

        for thread in ["one", "two"] {
            let path = directory.path().join(format!("{thread}.jsonl"));
            let partial = "x".repeat(200);
            fs::write(&path, format!("{}{}", header(thread, "parent"), partial)).unwrap();
            let publication = {
                let source = source(&path, thread);
                let staged = observer
                    .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
                    .unwrap();
                let publication = staged.publication().clone();
                staged.commit().unwrap();
                publication
            };
            if thread == "one" {
                assert!(matches!(publication, LifecyclePublication::Pending { .. }));
                assert_eq!(observer.cached_partial_bytes(), 200);
            } else {
                assert!(matches!(
                    publication,
                    LifecyclePublication::Pending {
                        reason: LifecyclePendingReason::PartialCapacity,
                        ..
                    }
                ));
                assert_eq!(observer.cached_partial_bytes(), 200);
            }
        }
    }

    #[test]
    fn header_thread_and_parent_must_match_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        fs::write(
            &path,
            format!(
                "{}{}",
                header("different-child", "different-parent"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
            ),
        )
        .unwrap();
        let source = source(&path, "child");
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::for_pass();

        assert!(matches!(
            stage(
                &mut observer,
                &source,
                &mut budget,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Rejected {
                reason: LifecycleRejection::ThreadIdentityMismatch,
                ..
            }
        ));
        assert!(observer
            .checkpoints
            .values()
            .next()
            .unwrap()
            .fold
            .state
            .is_none());

        fs::write(
            &path,
            format!(
                "{}{}",
                header("child", "wrong-parent"),
                event("task_complete", "T1", "2026-10-04T21:14:00Z"),
            ),
        )
        .unwrap();
        let mut parent_observer = LifecycleObserver::new();
        let mut parent_budget = LifecycleReadBudget::for_pass();
        assert!(matches!(
            stage(
                &mut parent_observer,
                &source,
                &mut parent_budget,
                CurrentNativePresence::Unobserved,
            ),
            LifecyclePublication::Rejected {
                reason: LifecycleRejection::ParentIdentityMismatch,
                ..
            }
        ));
    }
}
