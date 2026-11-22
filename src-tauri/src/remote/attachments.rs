//! Durable, path-only remote attachments. Call blocking operations on `spawn_blocking`.
//!
//! The caller authenticates before reserving/reading and rechecks current authority
//! before completing an upload and fencing dispatch. Workspace paths come only from
//! trusted agent configuration. One store owns the home for its entire lifetime.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const DEVICE_BYTES: u64 = 50 * 1024 * 1024;
const GLOBAL_BYTES: u64 = 200 * 1024 * 1024;
const RETAINED_BYTES: u64 = 1024 * 1024 * 1024;
const PENDING_TTL: u64 = 3600;
const RETAINED_TTL: u64 = 7 * 86400;
const MAX_SUBMISSIONS: usize = 2048;
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const NEW_SUBMISSION_AGE_MS: u64 = 5 * 60 * 1000;
const SUBMISSION_FUTURE_MS: u64 = 60 * 1000;
const SUBMISSION_PROOF_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Safe to expose remotely: never contains names, filesystem paths or OS errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentError {
    NotFound,
    InvalidId,
    InvalidFilename,
    UnsupportedType,
    InvalidContent,
    InvalidImage,
    FileTooLarge,
    QuotaExceeded,
    Busy,
    Conflict,
    Expired,
    SubmissionExpired,
    DiskFailure,
    DispatchFenced,
}
impl AttachmentError {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFound => "attachment_not_found",
            Self::InvalidId => "invalid_attachment_id",
            Self::InvalidFilename => "invalid_filename",
            Self::UnsupportedType => "unsupported_format",
            Self::InvalidContent => "invalid_content",
            Self::InvalidImage => "invalid_image",
            Self::FileTooLarge => "file_too_large",
            Self::QuotaExceeded => "attachment_quota",
            Self::Busy => "attachment_busy",
            Self::Conflict => "attachment_conflict",
            Self::Expired => "attachment_expired",
            Self::SubmissionExpired => "submission_expired",
            Self::DiskFailure => "attachment_storage_failure",
            Self::DispatchFenced => "submission_dispatch_fenced",
        }
    }
}
impl fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for AttachmentError {}
type Result<T> = std::result::Result<T, AttachmentError>;
fn disk<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|_| AttachmentError::DiskFailure)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Submission keys carry issuance time so an expired/pruned proof cannot be
/// recreated as a new request. Parse and future-check even for existing keys.
fn submission_issued_at(id: &str, timestamp_ms: u64) -> Result<u64> {
    if id.len() > 57 {
        return Err(AttachmentError::InvalidId);
    }
    let (epoch, uuid) = id.split_once('.').ok_or(AttachmentError::InvalidId)?;
    let issued = epoch
        .parse::<u64>()
        .map_err(|_| AttachmentError::InvalidId)?;
    if issued == 0
        || issued.to_string() != epoch
        || issued > timestamp_ms.saturating_add(SUBMISSION_FUTURE_MS)
    {
        return Err(AttachmentError::InvalidId);
    }
    valid_id(uuid)?;
    Ok(issued)
}
fn fresh_submission(issued_ms: u64, timestamp_ms: u64) -> Result<()> {
    if timestamp_ms.saturating_sub(issued_ms) > NEW_SUBMISSION_AGE_MS {
        return Err(AttachmentError::SubmissionExpired);
    }
    Ok(())
}
fn payload_hash(owner: &AttachmentOwner, prompt: &str, ids: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"wardian.remote.attachments.submission.v1\0");
    // Length prefixes preserve exact strings, ordered IDs and ownership without
    // ambiguous concatenation or retaining the original prompt in tombstones.
    for part in [
        owner.device_id.as_str(),
        owner.origin.as_str(),
        owner.agent_id.as_str(),
        prompt,
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hash.update((ids.len() as u64).to_be_bytes());
    for id in ids {
        hash.update((id.len() as u64).to_be_bytes());
        hash.update(id.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn valid_id(id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).map_err(|_| AttachmentError::InvalidId)?;
    if uuid.get_version_num() != 4
        || uuid.get_variant() != uuid::Variant::RFC4122
        || uuid.to_string() != id
    {
        return Err(AttachmentError::InvalidId);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentOwner {
    pub device_id: String,
    pub origin: String,
    pub agent_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub upload_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub expires_at: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    Prepared,
    Dispatching,
    Submitted,
    SubmittedUnconfirmed,
    FailedBeforeDispatch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    Submitted,
    SubmittedUnconfirmed,
    FailedBeforeDispatch,
}
#[derive(Debug, Clone)]
pub struct PreparedSubmission {
    pub prompt: String,
    pub status: SubmissionStatus,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    metadata: AttachmentMetadata,
    owner: AttachmentOwner,
    workspace: PathBuf,
    extension: String,
    digest: String,
    submission_id: Option<String>,
    pending_expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_claim: Option<RetainedClaim>,
}
#[derive(Clone, Serialize, Deserialize)]
struct RetainedClaim {
    submission_id: String,
    expires_at: u64,
}

/// A proven pre-dispatch failure preserves any earlier retention promise.
fn release_claim(record: &mut Record) {
    if let Some(prior) = record.rollback_claim.take() {
        record.submission_id = Some(prior.submission_id);
        record.metadata.expires_at = prior.expires_at;
    } else {
        record.submission_id = None;
        record.metadata.expires_at = record.pending_expires_at;
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Submission {
    owner: AttachmentOwner,
    payload_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachment_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_prompt: Option<String>,
    status: SubmissionStatus,
    expires_at_ms: u64,
}
impl Submission {
    fn terminal(&self) -> bool {
        !matches!(
            self.status,
            SubmissionStatus::Prepared | SubmissionStatus::Dispatching
        )
    }
    fn compact(&mut self) {
        if self.terminal() {
            self.resolved_prompt = None;
            self.attachment_ids.clear();
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Index {
    records: HashMap<String, Record>,
    submissions: HashMap<String, Submission>,
    roots: HashSet<PathBuf>,
}
struct Reservation {
    owner: AttachmentOwner,
    token: Uuid,
    cancelled: bool,
}
struct State {
    index: Index,
    reservations: HashMap<String, Reservation>,
    failed: bool,
}
/// Process-wide singleton recommended. Exclusive OS lock rejects a second opener.
pub struct AttachmentStore {
    root: PathBuf,
    state: Mutex<State>,
    _lock: File,
}
/// Capacity remains charged until completion or drop, including cancelled uploads.
pub struct UploadReservation {
    store: Arc<AttachmentStore>,
    owner: AttachmentOwner,
    workspace: PathBuf,
    upload_id: String,
    token: Uuid,
}
impl Drop for UploadReservation {
    fn drop(&mut self) {
        if let Ok(mut s) = self.store.state.lock() {
            if s.reservations
                .get(&self.upload_id)
                .is_some_and(|r| r.token == self.token)
            {
                s.reservations.remove(&self.upload_id);
            }
        }
    }
}

impl AttachmentStore {
    /// Opens the canonical index, recovers ambiguous dispatches and sweeps old bytes.
    pub fn open(wardian_home: &Path) -> Result<Arc<Self>> {
        let home = disk(wardian_home.canonicalize())?;
        let root = safe_dirs(&home, &["remote", "attachments"])?;
        let lock_path = root.join("store.lock");
        reject_link_if_exists(&lock_path)?;
        let lock = private_open(&lock_path, false)?;
        lock.try_lock_exclusive()
            .map_err(|_| AttachmentError::Busy)?;
        let index_path = root.join("index.json");
        reject_link_if_exists(&index_path)?;
        let mut index: Index = match File::open(&index_path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                disk(file.take(MAX_INDEX_BYTES + 1).read_to_end(&mut bytes))?;
                if bytes.len() as u64 > MAX_INDEX_BYTES {
                    return Err(AttachmentError::QuotaExceeded);
                }
                serde_json::from_slice(&bytes).map_err(|_| AttachmentError::DiskFailure)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Index::default(),
            Err(_) => return Err(AttachmentError::DiskFailure),
        };
        for submission in index.submissions.values_mut() {
            if submission.status == SubmissionStatus::Dispatching {
                submission.status = SubmissionStatus::SubmittedUnconfirmed;
            }
            submission.compact();
        }
        check_index_bounds(&index)?;
        let store = Arc::new(Self {
            root,
            state: Mutex::new(State {
                index,
                reservations: HashMap::new(),
                failed: false,
            }),
            _lock: lock,
        });
        {
            let s = store.lock()?;
            store.persist(&s.index)?;
        }
        store.cleanup()?;
        Ok(store)
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        let state = self
            .state
            .lock()
            .map_err(|_| AttachmentError::DiskFailure)?;
        if state.failed {
            return Err(AttachmentError::DiskFailure);
        }
        Ok(state)
    }
    fn persist(&self, index: &Index) -> Result<()> {
        check_index_bounds(index)?;
        check_chain(&self.root)?;
        reject_link_if_exists(&self.root.join("index.json"))?;
        let mut temp = disk(
            tempfile::Builder::new()
                .prefix(".attachment-index-")
                .tempfile_in(&self.root),
        )?;
        serde_json::to_writer(&mut temp, index).map_err(|_| AttachmentError::DiskFailure)?;
        disk(temp.as_file().sync_all())?;
        temp.persist(self.root.join("index.json"))
            .map_err(|_| AttachmentError::DiskFailure)?;
        sync_dir(&self.root)?;
        Ok(())
    }
    fn update(&self, state: &mut State, index: Index) -> Result<()> {
        // Quota rejection occurs before touching disk and is never an ambiguous
        // publication failure. Keep the store usable after a rejected admission.
        check_index_bounds(&index)?;
        if let Err(error) = self.persist(&index) {
            // A replace followed by a failed directory sync has ambiguous durability.
            // Require reopen/recovery rather than reusing stale in-memory state.
            state.failed = true;
            return Err(error);
        }
        state.index = index;
        Ok(())
    }
    /// Reserve a worst-case 10 MiB before consuming any authenticated request body.
    /// Existing IDs still require validation of the entire retry body at completion.
    pub fn reserve(
        self: &Arc<Self>,
        owner: &AttachmentOwner,
        workspace: &Path,
        upload_id: &str,
    ) -> Result<UploadReservation> {
        valid_id(upload_id)?;
        check_chain(workspace)?;
        let workspace = disk(workspace.canonicalize())?;
        if workspace
            .to_str()
            .is_none_or(|p| p.chars().any(char::is_control))
        {
            return Err(AttachmentError::DiskFailure);
        }
        let mut s = self.lock()?;
        if let Some(r) = s.index.records.get(upload_id) {
            if r.owner != *owner {
                return Err(AttachmentError::NotFound);
            }
            if r.workspace != workspace {
                return Err(AttachmentError::Conflict);
            }
            if r.metadata.expires_at <= now() {
                return Err(AttachmentError::Expired);
            }
        }
        if let Some(r) = s.reservations.get(upload_id) {
            return Err(if r.owner == *owner {
                AttachmentError::Busy
            } else {
                AttachmentError::NotFound
            });
        }
        if s.reservations
            .values()
            .filter(|r| r.owner.device_id == owner.device_id)
            .count()
            >= 2
        {
            return Err(AttachmentError::QuotaExceeded);
        }
        let pending: Vec<_> = s
            .index
            .records
            .values()
            .filter(|r| counts_as_pending(&s.index, r))
            .collect();
        let device = pending
            .iter()
            .filter(|r| r.owner.device_id == owner.device_id);
        let active = s
            .reservations
            .values()
            .filter(|r| r.owner.device_id == owner.device_id)
            .count() as u64;
        if (!s.index.records.contains_key(upload_id) && device.clone().count() as u64 + active >= 8)
            || device.map(|r| r.metadata.size_bytes).sum::<u64>() + (active + 1) * MAX_FILE_BYTES
                > DEVICE_BYTES
            || pending.iter().map(|r| r.metadata.size_bytes).sum::<u64>()
                + (s.reservations.len() as u64 + 1) * MAX_FILE_BYTES
                > GLOBAL_BYTES
        {
            return Err(AttachmentError::QuotaExceeded);
        }
        let root = safe_dirs(&workspace, &[".wardian", "attachments", "remote"])?;
        protect_upload_root(&root)?;
        let mut next = s.index.clone();
        next.roots.insert(root);
        self.update(&mut s, next)?;
        let token = Uuid::new_v4();
        s.reservations.insert(
            upload_id.to_owned(),
            Reservation {
                owner: owner.clone(),
                token,
                cancelled: false,
            },
        );
        Ok(UploadReservation {
            store: self.clone(),
            owner: owner.clone(),
            workspace,
            upload_id: upload_id.to_owned(),
            token,
        })
    }
    /// Returns owned metadata only; never returns a server path or file bytes.
    pub fn status(&self, owner: &AttachmentOwner, upload_id: &str) -> Result<AttachmentMetadata> {
        let s = self.lock()?;
        let r = owned(&s.index, owner, upload_id)?;
        if r.metadata.expires_at <= now() {
            return Err(AttachmentError::Expired);
        }
        Ok(r.metadata.clone())
    }
    /// Unknown IDs are idempotent; foreign IDs are indistinguishable from unknown.
    pub fn delete(&self, owner: &AttachmentOwner, upload_id: &str) -> Result<()> {
        let mut s = self.lock()?;
        if let Some(r) = s
            .reservations
            .get_mut(upload_id)
            .filter(|r| r.owner == *owner)
        {
            r.cancelled = true;
        }
        let Some(r) = s
            .index
            .records
            .get(upload_id)
            .filter(|r| r.owner == *owner)
            .cloned()
        else {
            return Ok(());
        };
        if r.submission_id.is_some() {
            return Err(AttachmentError::Conflict);
        }
        remove_record_bytes(&r)?;
        let mut next = s.index.clone();
        next.records.remove(upload_id);
        self.update(&mut s, next)
    }
    /// Atomically claims all IDs and retains them for seven days. No provider writes.
    /// Keys are `<issued_epoch_ms>.<canonical UUIDv4>`. Unknown keys must be at
    /// most five minutes old (60 seconds future tolerance). Existing proofs have
    /// a fixed 24-hour lifetime; terminal duplicates return status and an empty prompt.
    pub fn prepare_submission(
        &self,
        owner: &AttachmentOwner,
        submission_id: &str,
        prompt: &str,
        attachment_ids: &[String],
    ) -> Result<PreparedSubmission> {
        self.prepare_submission_at(owner, submission_id, prompt, attachment_ids, now_ms())
    }
    fn prepare_submission_at(
        &self,
        owner: &AttachmentOwner,
        submission_id: &str,
        prompt: &str,
        attachment_ids: &[String],
        timestamp_ms: u64,
    ) -> Result<PreparedSubmission> {
        let issued_ms = submission_issued_at(submission_id, timestamp_ms)?;
        let mut s = self.lock()?;
        if s.index.submissions.contains_key(submission_id) {
            let old = owned_submission_at(&s.index, owner, submission_id, timestamp_ms)?;
            if prompt.len() as u64 > MAX_INDEX_BYTES
                || old.payload_hash != payload_hash(owner, prompt, attachment_ids)
            {
                return Err(AttachmentError::Conflict);
            }
            return prepared(old);
        }
        fresh_submission(issued_ms, timestamp_ms)?;
        if attachment_ids.is_empty()
            || attachment_ids.len() > 4
            || attachment_ids.iter().collect::<HashSet<_>>().len() != attachment_ids.len()
        {
            return Err(AttachmentError::InvalidContent);
        }
        if s.index.submissions.len() >= MAX_SUBMISSIONS || prompt.len() as u64 > MAX_INDEX_BYTES {
            return Err(AttachmentError::QuotaExceeded);
        }
        let mut size = 0;
        let mut newly_retained_bytes = 0;
        let mut paths = Vec::new();
        for id in attachment_ids {
            let r = owned(&s.index, owner, id)?;
            if let Some(prior_id) = &r.submission_id {
                if let Some(prior) = s.index.submissions.get(prior_id) {
                    // Only an explicit new key may reuse retained files after the
                    // prior terminal proof expires. Never reclaim a live fence.
                    if prior.owner != *owner
                        || !prior.terminal()
                        || prior.expires_at_ms > timestamp_ms
                    {
                        return Err(AttachmentError::Conflict);
                    }
                }
                // A missing proof was pruned after terminal expiry. The file's
                // association keeps it retained without charging its bytes twice.
            } else {
                newly_retained_bytes += r.metadata.size_bytes;
            }
            if r.metadata.expires_at <= timestamp_ms / 1000 {
                return Err(AttachmentError::Expired);
            }
            if s.reservations.contains_key(id) {
                return Err(AttachmentError::Busy);
            }
            let path = record_path(r);
            check_chain(&path)?;
            if !disk(fs::metadata(&path))?.is_file() {
                return Err(AttachmentError::DiskFailure);
            }
            size += r.metadata.size_bytes;
            paths.push(path);
        }
        if size > 25 * 1024 * 1024
            || s.index
                .records
                .values()
                .filter(|r| r.submission_id.is_some())
                .map(|r| r.metadata.size_bytes)
                .sum::<u64>()
                + newly_retained_bytes
                > RETAINED_BYTES
        {
            return Err(AttachmentError::QuotaExceeded);
        }
        let body = if prompt.trim().is_empty() {
            "Please inspect the attached files."
        } else {
            prompt
        };
        let resolved_prompt = format!(
            "{}\n\nAttached files:\n{}",
            body,
            paths
                .iter()
                .map(|p| format!("- {}", provider_reference(p)))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let sub = Submission {
            owner: owner.clone(),
            payload_hash: payload_hash(owner, prompt, attachment_ids),
            attachment_ids: attachment_ids.to_vec(),
            resolved_prompt: Some(resolved_prompt),
            status: SubmissionStatus::Prepared,
            expires_at_ms: timestamp_ms.saturating_add(SUBMISSION_PROOF_TTL_MS),
        };
        let mut next = s.index.clone();
        for id in attachment_ids {
            let r = next.records.get_mut(id).ok_or(AttachmentError::NotFound)?;
            r.rollback_claim = r.submission_id.as_ref().map(|id| RetainedClaim {
                submission_id: id.clone(),
                expires_at: r.metadata.expires_at,
            });
            r.submission_id = Some(submission_id.to_owned());
            r.metadata.expires_at = (timestamp_ms / 1000).saturating_add(RETAINED_TTL);
        }
        next.submissions
            .insert(submission_id.to_owned(), sub.clone());
        self.update(&mut s, next)?;
        prepared(&sub)
    }
    /// Persist this fence immediately before the first irreversible provider write.
    /// Every repeated call fails closed, even within the same process.
    pub fn begin_dispatch(
        &self,
        owner: &AttachmentOwner,
        submission_id: &str,
    ) -> Result<SubmissionStatus> {
        let mut s = self.lock()?;
        let old = owned_submission(&s.index, owner, submission_id)?;
        if old.status != SubmissionStatus::Prepared {
            return Err(AttachmentError::DispatchFenced);
        }
        for id in &old.attachment_ids {
            let r = owned(&s.index, owner, id)?;
            if r.metadata.expires_at <= now() {
                return Err(AttachmentError::Expired);
            }
            check_chain(&record_path(r))?;
        }
        let mut next = s.index.clone();
        next.submissions
            .get_mut(submission_id)
            .ok_or(AttachmentError::NotFound)?
            .status = SubmissionStatus::Dispatching;
        self.update(&mut s, next)?;
        Ok(SubmissionStatus::Dispatching)
    }
    /// `FailedBeforeDispatch` asserts positive proof that no provider side effect
    /// occurred. It releases claims for a NEW explicit submission ID; old IDs never replay.
    pub fn finish_submission(
        &self,
        owner: &AttachmentOwner,
        submission_id: &str,
        outcome: DispatchOutcome,
    ) -> Result<SubmissionStatus> {
        let status = match outcome {
            DispatchOutcome::Submitted => SubmissionStatus::Submitted,
            DispatchOutcome::SubmittedUnconfirmed => SubmissionStatus::SubmittedUnconfirmed,
            DispatchOutcome::FailedBeforeDispatch => SubmissionStatus::FailedBeforeDispatch,
        };
        let mut s = self.lock()?;
        let old = owned_submission(&s.index, owner, submission_id)?.clone();
        if old.status == status {
            return Ok(status);
        }
        if old.status != SubmissionStatus::Dispatching
            && !(old.status == SubmissionStatus::Prepared
                && status == SubmissionStatus::FailedBeforeDispatch)
        {
            return Err(AttachmentError::Conflict);
        }
        let mut next = s.index.clone();
        next.submissions
            .get_mut(submission_id)
            .ok_or(AttachmentError::NotFound)?
            .status = status;
        for id in &old.attachment_ids {
            if let Some(record) = next.records.get_mut(id) {
                if status == SubmissionStatus::FailedBeforeDispatch {
                    release_claim(record);
                } else {
                    record.rollback_claim = None;
                }
            }
        }
        next.submissions
            .get_mut(submission_id)
            .ok_or(AttachmentError::NotFound)?
            .compact();
        self.update(&mut s, next)?;
        Ok(status)
    }
    /// Recovers the durable delivery state without authorizing another dispatch.
    pub fn submission_status(
        &self,
        owner: &AttachmentOwner,
        submission_id: &str,
    ) -> Result<SubmissionStatus> {
        Ok(owned_submission(&self.lock()?.index, owner, submission_id)?.status)
    }
    /// Revocation deletes unsubmitted bytes. Retained files keep their retention promise.
    /// Gateway authority must continue rejecting this device on every operation.
    pub fn revoke_device(&self, device_id: &str) -> Result<()> {
        let mut s = self.lock()?;
        for r in s
            .reservations
            .values_mut()
            .filter(|r| r.owner.device_id == device_id)
        {
            r.cancelled = true;
        }
        let mut next = s.index.clone();
        for (id, r) in &s.index.records {
            if r.owner.device_id == device_id && r.submission_id.is_none() {
                remove_record_bytes(r)?;
                next.records.remove(id);
            }
        }
        self.update(&mut s, next)
    }
    /// Nonrecursive cleanup: never traverses a link or removes arbitrary workspace data.
    pub fn cleanup(&self) -> Result<()> {
        self.cleanup_at(now())
    }
    fn cleanup_at(&self, timestamp: u64) -> Result<()> {
        let mut s = self.lock()?;
        let mut next = s.index.clone();
        let timestamp_ms = timestamp.saturating_mul(1000);
        let expired_prepared: HashSet<_> = next
            .submissions
            .iter()
            .filter(|(_, sub)| {
                sub.status == SubmissionStatus::Prepared && sub.expires_at_ms <= timestamp_ms
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired_prepared {
            let sub = next
                .submissions
                .get_mut(id)
                .ok_or(AttachmentError::NotFound)?;
            sub.status = SubmissionStatus::FailedBeforeDispatch;
            sub.compact();
        }
        for record in next.records.values_mut() {
            if record
                .submission_id
                .as_ref()
                .is_some_and(|id| expired_prepared.contains(id))
            {
                release_claim(record);
            }
        }
        next.submissions
            .retain(|_, sub| !sub.terminal() || sub.expires_at_ms > timestamp_ms);
        let record_ids: Vec<_> = next.records.keys().cloned().collect();
        for id in record_ids {
            let r = &next.records[&id];
            let pinned = r
                .submission_id
                .as_ref()
                .and_then(|id| next.submissions.get(id))
                .is_some_and(|s| s.status == SubmissionStatus::Dispatching);
            if r.metadata.expires_at <= timestamp && !pinned && !s.reservations.contains_key(&id) {
                remove_record_bytes(r)?;
                next.records.remove(&id);
            }
        }
        check_chain(&self.root)?;
        for entry in disk(fs::read_dir(&self.root))? {
            let entry = disk(entry)?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".attachment-index-")
            {
                check_chain(&entry.path())?;
                if disk(entry.metadata())?.is_file() {
                    disk(fs::remove_file(entry.path()))?;
                }
            }
        }
        // Registered roots are persisted before any blob write, covering crash orphans.
        for root in &s.index.roots {
            if !root
                .try_exists()
                .map_err(|_| AttachmentError::DiskFailure)?
            {
                next.roots.remove(root);
                continue;
            }
            check_chain(root)?;
            protect_upload_root(root)?;
            for entry in disk(fs::read_dir(root))? {
                let entry = disk(entry)?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if valid_id(&name).is_err()
                    || next.records.contains_key(&name)
                    || s.reservations.contains_key(&name)
                {
                    continue;
                }
                remove_upload_dir(&entry.path())?;
            }
        }
        self.update(&mut s, next)
    }
}

impl UploadReservation {
    /// Validates capped bytes, then publishes bytes and durable metadata. Caller must
    /// recheck live authority immediately before calling. No client path is accepted.
    pub fn complete(
        self,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<AttachmentMetadata> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(AttachmentError::FileTooLarge);
        }
        let extension = validate(name, media_type, bytes)?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        let mut s = self.store.lock()?;
        if !s
            .reservations
            .get(&self.upload_id)
            .is_some_and(|r| r.owner == self.owner && r.token == self.token && !r.cancelled)
        {
            return Err(AttachmentError::NotFound);
        }
        if let Some(r) = s.index.records.get(&self.upload_id) {
            if r.owner != self.owner {
                return Err(AttachmentError::NotFound);
            }
            if r.metadata.expires_at <= now() {
                return Err(AttachmentError::Expired);
            }
            if r.metadata.name != name || r.metadata.media_type != media_type || r.digest != digest
            {
                return Err(AttachmentError::Conflict);
            }
            return Ok(r.metadata.clone());
        }
        let metadata = AttachmentMetadata {
            upload_id: self.upload_id.clone(),
            name: name.to_owned(),
            size_bytes: bytes.len() as u64,
            media_type: media_type.to_owned(),
            expires_at: now() + PENDING_TTL,
        };
        let r = Record {
            metadata: metadata.clone(),
            owner: self.owner.clone(),
            workspace: self.workspace.clone(),
            extension,
            digest,
            submission_id: None,
            pending_expires_at: metadata.expires_at,
            rollback_claim: None,
        };
        let mut next = s.index.clone();
        next.records.insert(self.upload_id.clone(), r.clone());
        // Admission must precede even directory/blob publication. Otherwise a
        // full metadata ledger permits repeated quota-rejected orphan writes.
        check_index_bounds(&next)?;
        let root = safe_dirs(&self.workspace, &[".wardian", "attachments", "remote"])?;
        protect_upload_root(&root)?;
        let dir = root.join(&self.upload_id);
        // A crash orphan may remain from an earlier uncommitted attempt.
        if dir.try_exists().map_err(|_| AttachmentError::DiskFailure)? {
            remove_upload_dir(&dir)?;
        }
        disk(fs::create_dir(&dir))?;
        private_dir(&dir)?;
        sync_dir(&root)?;
        let result = (|| {
            let mut tmp = disk(tempfile::NamedTempFile::new_in(&dir))?;
            disk(tmp.write_all(bytes))?;
            disk(tmp.as_file().sync_all())?;
            check_chain(&dir)?;
            tmp.persist_noclobber(record_path(&r))
                .map_err(|_| AttachmentError::DiskFailure)?;
            sync_dir(&dir)?;
            self.store.update(&mut s, next)
        })();
        // Preserve crash orphans on ambiguous publication errors. The persisted
        // root registry lets cleanup reclaim them without deleting committed bytes.
        result?;
        Ok(metadata)
    }
}
// Rollback capacity stays reserved until a terminal dispatch outcome. Preparing
// files cannot make room for uploads that would overflow quotas on proven failure.
fn counts_as_pending(index: &Index, record: &Record) -> bool {
    match &record.submission_id {
        None => true,
        Some(id) => index.submissions.get(id).is_some_and(|s| {
            matches!(
                s.status,
                SubmissionStatus::Prepared
                    | SubmissionStatus::Dispatching
                    | SubmissionStatus::FailedBeforeDispatch
            )
        }),
    }
}

fn owned<'a>(index: &'a Index, owner: &AttachmentOwner, id: &str) -> Result<&'a Record> {
    index
        .records
        .get(id)
        .filter(|r| r.owner == *owner)
        .ok_or(AttachmentError::NotFound)
}
fn owned_submission<'a>(
    index: &'a Index,
    owner: &AttachmentOwner,
    id: &str,
) -> Result<&'a Submission> {
    owned_submission_at(index, owner, id, now_ms())
}
fn owned_submission_at<'a>(
    index: &'a Index,
    owner: &AttachmentOwner,
    id: &str,
    timestamp_ms: u64,
) -> Result<&'a Submission> {
    let issued_ms = submission_issued_at(id, timestamp_ms)?;
    let Some(sub) = index.submissions.get(id) else {
        fresh_submission(issued_ms, timestamp_ms)?;
        return Err(AttachmentError::NotFound);
    };
    if timestamp_ms >= sub.expires_at_ms {
        return Err(AttachmentError::SubmissionExpired);
    }
    if sub.owner != *owner {
        return Err(AttachmentError::NotFound);
    }
    Ok(sub)
}

/// Count the exact serialized index with a bounded writer. This includes the
/// ledger, file metadata and registered roots, rather than bounding only prompts.
fn check_index_bounds(index: &Index) -> Result<()> {
    if index.submissions.len() > MAX_SUBMISSIONS {
        return Err(AttachmentError::QuotaExceeded);
    }
    struct Counter(u64);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            if self.0 > MAX_INDEX_BYTES {
                return Err(std::io::Error::other("attachment_index_limit"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), index).map_err(|_| AttachmentError::QuotaExceeded)
}

fn prepared(sub: &Submission) -> Result<PreparedSubmission> {
    Ok(PreparedSubmission {
        prompt: sub.resolved_prompt.clone().unwrap_or_default(),
        status: sub.status,
    })
}
// Normalize only display references: providers expect the same conventional paths
// as desktop attachments; filesystem checks retain canonical verbatim paths.
fn provider_reference(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{stripped}");
        }
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return stripped.to_owned();
        }
    }
    text.into_owned()
}
fn record_path(r: &Record) -> PathBuf {
    r.workspace
        .join(".wardian/attachments/remote")
        .join(&r.metadata.upload_id)
        .join(format!("{}.{}", r.metadata.upload_id, r.extension))
}
fn remove_record_bytes(r: &Record) -> Result<()> {
    remove_upload_dir(
        &r.workspace
            .join(".wardian/attachments/remote")
            .join(&r.metadata.upload_id),
    )
}
fn remove_upload_dir(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(AttachmentError::DiskFailure),
        Ok(_) => (),
    }
    check_chain(dir)?;
    for entry in disk(fs::read_dir(dir))? {
        let path = disk(entry)?.path();
        check_chain(&path)?;
        if !disk(fs::metadata(&path))?.is_file() {
            return Err(AttachmentError::DiskFailure);
        }
        disk(fs::remove_file(&path))?;
    }
    disk(fs::remove_dir(dir))
}
fn check_chain(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let m = disk(fs::symlink_metadata(ancestor))?;
        if is_link(&m) {
            return Err(AttachmentError::DiskFailure);
        }
    }
    Ok(())
}
fn is_link(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
fn reject_link_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if is_link(&m) || !m.is_file() => Err(AttachmentError::DiskFailure),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AttachmentError::DiskFailure),
    }
}
/// Keep temporary user bytes out of ordinary Git staging without changing a
/// project's ignore rules. A foreign or linked marker fails closed, never gets
/// overwritten, and cannot silently expose uploads through negation patterns.
fn protect_upload_root(root: &Path) -> Result<()> {
    let marker = root.join(".gitignore");
    reject_link_if_exists(&marker)?;
    match File::open(&marker) {
        Ok(file) => {
            let mut contents = Vec::new();
            disk(file.take(4).read_to_end(&mut contents))?;
            if contents != b"*\n" && contents != b"*\r\n" {
                return Err(AttachmentError::DiskFailure);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = private_open(&marker, true)?;
            disk(file.write_all(b"*\n"))?;
            disk(file.sync_all())?;
            sync_dir(root)?;
        }
        Err(_) => return Err(AttachmentError::DiskFailure),
    }
    Ok(())
}

fn safe_dirs(base: &Path, components: &[&str]) -> Result<PathBuf> {
    check_chain(base)?;
    let mut dir = base.to_path_buf();
    for c in components {
        dir.push(c);
        match fs::create_dir(&dir) {
            Ok(()) => {
                private_dir(&dir)?;
                sync_dir(dir.parent().ok_or(AttachmentError::DiskFailure)?)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(AttachmentError::DiskFailure),
        }
        check_chain(&dir)?;
        if !disk(fs::metadata(&dir))?.is_dir() {
            return Err(AttachmentError::DiskFailure);
        }
    }
    Ok(dir)
}
fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        disk(fs::set_permissions(path, fs::Permissions::from_mode(0o700)))?;
    }
    let _ = path;
    Ok(())
}
fn private_open(path: &Path, exclusive: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    disk(options.open(path))
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        disk(File::open(path).and_then(|f| f.sync_all()))?;
    }
    let _ = path;
    Ok(())
}

fn validate(name: &str, media_type: &str, bytes: &[u8]) -> Result<String> {
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
        || name.split('.').any(|p| p.is_empty())
    {
        return Err(AttachmentError::InvalidFilename);
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let expected = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        _ => return Err(AttachmentError::UnsupportedType),
    };
    if media_type != expected {
        return Err(AttachmentError::UnsupportedType);
    }
    if expected.starts_with("image/") {
        use image::{ImageFormat, ImageReader, Limits};
        let format = match ext.as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "webp" => ImageFormat::WebP,
            "gif" => ImageFormat::Gif,
            _ => ImageFormat::Bmp,
        };
        if image::guess_format(bytes).ok() != Some(format) {
            return Err(AttachmentError::InvalidImage);
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut header_limits = Limits::default();
        header_limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(header_limits);
        let (w, h) = reader
            .into_dimensions()
            .map_err(|_| AttachmentError::InvalidImage)?;
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 25_000_000 {
            return Err(AttachmentError::InvalidImage);
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(w);
        limits.max_image_height = Some(h);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits.clone());
        reader.decode().map_err(|_| AttachmentError::InvalidImage)?;
        // Validate all animated frames as well, bounding cumulative decoded work.
        use image::{AnimationDecoder, ImageDecoder};
        match format {
            ImageFormat::Gif => {
                let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                    .map_err(|_| AttachmentError::InvalidImage)?;
                decoder
                    .set_limits(limits)
                    .map_err(|_| AttachmentError::InvalidImage)?;
                validate_frames(decoder.into_frames())?;
            }
            ImageFormat::WebP => {
                let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                    .map_err(|_| AttachmentError::InvalidImage)?;
                decoder
                    .set_limits(limits)
                    .map_err(|_| AttachmentError::InvalidImage)?;
                if decoder.has_animation() {
                    validate_frames(decoder.into_frames())?;
                }
            }
            ImageFormat::Png => {
                let decoder =
                    image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits)
                        .map_err(|_| AttachmentError::InvalidImage)?;
                if decoder
                    .is_apng()
                    .map_err(|_| AttachmentError::InvalidImage)?
                {
                    validate_frames(
                        decoder
                            .apng()
                            .map_err(|_| AttachmentError::InvalidImage)?
                            .into_frames(),
                    )?;
                }
            }
            _ => (),
        }
    } else if ext == "pdf" {
        if !bytes.starts_with(b"%PDF-") {
            return Err(AttachmentError::InvalidContent);
        }
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| AttachmentError::InvalidContent)?;
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(AttachmentError::InvalidContent);
        }
        if ext == "json" {
            serde_json::from_str::<serde_json::Value>(text)
                .map_err(|_| AttachmentError::InvalidContent)?;
        }
    }
    Ok(ext)
}

fn validate_frames(frames: image::Frames<'_>) -> Result<()> {
    let mut pixels = 0u64;
    for (i, frame) in frames.enumerate() {
        if i >= 1000 {
            return Err(AttachmentError::InvalidImage);
        }
        let frame = frame.map_err(|_| AttachmentError::InvalidImage)?;
        pixels += u64::from(frame.buffer().width()) * u64::from(frame.buffer().height());
        if pixels > 25_000_000 {
            return Err(AttachmentError::InvalidImage);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn submission_key() -> String {
        format!("{}.{}", now_ms(), Uuid::new_v4())
    }
    fn key_at(timestamp_ms: u64) -> String {
        format!("{timestamp_ms}.{}", Uuid::new_v4())
    }
    struct Fixture {
        _temp: tempfile::TempDir,
        home: PathBuf,
        workspace: PathBuf,
        store: Arc<AttachmentStore>,
        owner: AttachmentOwner,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let workspace = temp.path().join("workspace");
            fs::create_dir(&home).unwrap();
            fs::create_dir(&workspace).unwrap();
            let store = AttachmentStore::open(&home).unwrap();
            Self {
                _temp: temp,
                home,
                workspace,
                store,
                owner: AttachmentOwner {
                    device_id: "device".into(),
                    origin: "https://example.test".into(),
                    agent_id: "agent".into(),
                },
            }
        }
        fn reserve(&self, id: &str) -> UploadReservation {
            self.store
                .reserve(&self.owner, &self.workspace, id)
                .unwrap()
        }
        fn upload(&self) -> String {
            let id = Uuid::new_v4().to_string();
            self.reserve(&id)
                .complete("notes.txt", "text/plain", b"hello")
                .unwrap();
            id
        }
        fn prepare(&self, id: &str) -> String {
            let sid = submission_key();
            self.store
                .prepare_submission(&self.owner, &sid, "", &[id.to_owned()])
                .unwrap();
            sid
        }
    }
    #[test]
    fn submission_key_validation_and_unknown_stale_requests() {
        let f = Fixture::new();
        let id = f.upload();
        let timestamp = now_ms();
        for key in [
            Uuid::new_v4().to_string(),
            format!("0{timestamp}.{}", Uuid::new_v4()),
            format!("+{timestamp}.{}", Uuid::new_v4()),
            key_at(timestamp + SUBMISSION_FUTURE_MS + 1),
            format!("{timestamp}.00000000-0000-4000-0000-000000000000"),
        ] {
            assert_eq!(
                f.store
                    .prepare_submission_at(&f.owner, &key, "", std::slice::from_ref(&id), timestamp)
                    .unwrap_err(),
                AttachmentError::InvalidId
            );
        }
        let stale = key_at(timestamp - NEW_SUBMISSION_AGE_MS - 1);
        assert_eq!(
            f.store
                .prepare_submission_at(&f.owner, &stale, "", std::slice::from_ref(&id), timestamp)
                .unwrap_err(),
            AttachmentError::SubmissionExpired
        );
        assert!(matches!(
            owned_submission_at(&f.store.lock().unwrap().index, &f.owner, &stale, timestamp),
            Err(AttachmentError::SubmissionExpired)
        ));
        let future = key_at(timestamp + SUBMISSION_FUTURE_MS);
        f.store
            .prepare_submission_at(&f.owner, &future, "", &[id], timestamp)
            .unwrap();
        // Parsing must still run before a duplicate lookup, even if disk state
        // contains an otherwise matching proof issued too far in the future.
        assert_eq!(
            f.store
                .prepare_submission_at(&f.owner, &future, "", &[], timestamp - 1)
                .unwrap_err(),
            AttachmentError::InvalidId
        );
    }
    #[test]
    fn existing_proof_has_fixed_twenty_four_hour_recovery_window() {
        let f = Fixture::new();
        let id = f.upload();
        let timestamp = now_ms();
        let key = key_at(timestamp);
        let ids = [id];
        f.store
            .prepare_submission_at(&f.owner, &key, "exact prompt", &ids, timestamp)
            .unwrap();
        let later = timestamp + NEW_SUBMISSION_AGE_MS + 1;
        assert_eq!(
            f.store
                .prepare_submission_at(&f.owner, &key, "exact prompt", &ids, later)
                .unwrap()
                .status,
            SubmissionStatus::Prepared
        );
        assert_eq!(
            f.store.lock().unwrap().index.submissions[&key].expires_at_ms,
            timestamp + SUBMISSION_PROOF_TTL_MS
        );
        assert_eq!(
            f.store
                .prepare_submission_at(&f.owner, &key, "exact prompt ", &ids, later)
                .unwrap_err(),
            AttachmentError::Conflict
        );
        assert_eq!(
            f.store
                .prepare_submission_at(
                    &f.owner,
                    &key,
                    "exact prompt",
                    &ids,
                    timestamp + SUBMISSION_PROOF_TTL_MS
                )
                .unwrap_err(),
            AttachmentError::SubmissionExpired
        );
        assert!(matches!(
            owned_submission_at(
                &f.store.lock().unwrap().index,
                &f.owner,
                &key,
                timestamp + SUBMISSION_PROOF_TTL_MS
            ),
            Err(AttachmentError::SubmissionExpired)
        ));
    }
    #[test]
    fn terminal_compaction_preserves_exact_payload_conflict_proofs() {
        for outcome in [
            DispatchOutcome::Submitted,
            DispatchOutcome::SubmittedUnconfirmed,
            DispatchOutcome::FailedBeforeDispatch,
        ] {
            let f = Fixture::new();
            let ids = [f.upload(), f.upload()];
            let key = submission_key();
            let prompt = "private original prompt marker";
            f.store
                .prepare_submission(&f.owner, &key, prompt, &ids)
                .unwrap();
            f.store.begin_dispatch(&f.owner, &key).unwrap();
            let status = f.store.finish_submission(&f.owner, &key, outcome).unwrap();
            {
                let state = f.store.lock().unwrap();
                let sub = &state.index.submissions[&key];
                assert!(sub.resolved_prompt.is_none());
                assert!(sub.attachment_ids.is_empty());
                assert_eq!(sub.payload_hash.len(), 64);
            }
            let persisted = fs::read_to_string(f.store.root.join("index.json")).unwrap();
            assert!(!persisted.contains(prompt));
            assert!(!persisted.contains("Attached files:"));
            let duplicate = f
                .store
                .prepare_submission(&f.owner, &key, prompt, &ids)
                .unwrap();
            assert_eq!(duplicate.status, status);
            assert!(duplicate.prompt.is_empty());
            assert_eq!(
                f.store
                    .prepare_submission(&f.owner, &key, "changed", &ids)
                    .unwrap_err(),
                AttachmentError::Conflict
            );
            assert_eq!(
                f.store
                    .prepare_submission(&f.owner, &key, prompt, &[ids[1].clone(), ids[0].clone()])
                    .unwrap_err(),
                AttachmentError::Conflict
            );
            assert_eq!(
                f.store.begin_dispatch(&f.owner, &key).unwrap_err(),
                AttachmentError::DispatchFenced
            );
            let mut other_target = f.owner.clone();
            other_target.agent_id.push('x');
            assert_ne!(
                payload_hash(&f.owner, prompt, &ids),
                payload_hash(&other_target, prompt, &ids)
            );
        }
    }
    #[test]
    fn expired_terminal_proof_cleanup_and_restart_preserve_retained_files() {
        let mut f = Fixture::new();
        let id = f.upload();
        let key = f.prepare(&id);
        f.store.begin_dispatch(&f.owner, &key).unwrap();
        f.store
            .finish_submission(&f.owner, &key, DispatchOutcome::Submitted)
            .unwrap();
        let timestamp = now_ms() + SUBMISSION_PROOF_TTL_MS + 2000;
        f.store.cleanup_at(timestamp / 1000).unwrap();
        {
            let state = f.store.lock().unwrap();
            assert!(!state.index.submissions.contains_key(&key));
            let record = &state.index.records[&id];
            assert_eq!(record.submission_id.as_deref(), Some(key.as_str()));
            assert!(!counts_as_pending(&state.index, record));
            assert!(record_path(record).is_file());
        }
        assert_eq!(
            f.store
                .prepare_submission_at(&f.owner, &key, "", std::slice::from_ref(&id), timestamp)
                .unwrap_err(),
            AttachmentError::SubmissionExpired
        );
        drop(f.store);
        f.store = AttachmentStore::open(&f.home).unwrap();
        assert!(f.store.status(&f.owner, &id).is_ok());
        assert_eq!(
            f.store
                .prepare_submission_at(&f.owner, &key, "", &[id], timestamp)
                .unwrap_err(),
            AttachmentError::SubmissionExpired
        );
    }
    #[test]
    fn cleanup_releases_expired_prepared_claims_but_preserves_dispatch_fence() {
        let mut f = Fixture::new();
        let a = f.upload();
        let b = f.upload();
        let prepared = f.prepare(&a);
        let dispatching = f.prepare(&b);
        f.store.begin_dispatch(&f.owner, &dispatching).unwrap();
        {
            let mut state = f.store.lock().unwrap();
            let mut next = state.index.clone();
            next.submissions.get_mut(&prepared).unwrap().expires_at_ms =
                now_ms().saturating_sub(2000);
            next.submissions
                .get_mut(&dispatching)
                .unwrap()
                .expires_at_ms = now_ms().saturating_sub(2000);
            f.store.update(&mut state, next).unwrap();
        }
        f.store.cleanup().unwrap();
        {
            let state = f.store.lock().unwrap();
            assert_eq!(
                state.index.submissions[&dispatching].status,
                SubmissionStatus::Dispatching
            );
            assert!(state.index.submissions[&dispatching]
                .resolved_prompt
                .is_some());
            assert!(!state.index.submissions.contains_key(&prepared));
            assert!(state.index.records[&a].submission_id.is_none());
        }
        assert_eq!(
            f.store
                .submission_status(&f.owner, &dispatching)
                .unwrap_err(),
            AttachmentError::SubmissionExpired
        );
        drop(f.store);
        f.store = AttachmentStore::open(&f.home).unwrap();
        let state = f.store.lock().unwrap();
        assert!(!state.index.submissions.contains_key(&dispatching)); // restart resolves ambiguity, then expired terminal proof is reclaimable
        assert!(!state.index.submissions.contains_key(&prepared));
        assert!(state.index.records.contains_key(&b));
    }
    fn expire_terminal_claim(f: &Fixture, key: &str) -> String {
        let stale = key_at(now_ms() - SUBMISSION_PROOF_TTL_MS - 2000);
        let mut state = f.store.lock().unwrap();
        let mut next = state.index.clone();
        let mut proof = next.submissions.remove(key).unwrap();
        assert!(proof.terminal());
        proof.expires_at_ms = now_ms().saturating_sub(2000);
        next.submissions.insert(stale.clone(), proof);
        for record in next.records.values_mut() {
            if record.submission_id.as_deref() == Some(key) {
                record.submission_id = Some(stale.clone());
            }
        }
        f.store.update(&mut state, next).unwrap();
        stale
    }
    #[test]
    fn explicit_new_key_reclaims_retained_files_after_terminal_expiry_and_restart() {
        for restart in [false, true] {
            let mut f = Fixture::new();
            let id = f.upload();
            let prior = f.prepare(&id);
            f.store.begin_dispatch(&f.owner, &prior).unwrap();
            f.store
                .finish_submission(&f.owner, &prior, DispatchOutcome::Submitted)
                .unwrap();
            assert_eq!(
                f.store
                    .prepare_submission(
                        &f.owner,
                        &submission_key(),
                        "new intent",
                        std::slice::from_ref(&id)
                    )
                    .unwrap_err(),
                AttachmentError::Conflict
            );
            let old = expire_terminal_claim(&f, &prior);
            f.store.cleanup().unwrap();
            if restart {
                drop(f.store);
                f.store = AttachmentStore::open(&f.home).unwrap();
            }
            assert_eq!(
                f.store.submission_status(&f.owner, &old).unwrap_err(),
                AttachmentError::SubmissionExpired
            );
            assert_eq!(
                f.store
                    .prepare_submission(&f.owner, &old, "", std::slice::from_ref(&id))
                    .unwrap_err(),
                AttachmentError::SubmissionExpired
            );
            let new_key = submission_key();
            assert_eq!(
                f.store
                    .prepare_submission(&f.owner, &new_key, "new intent", std::slice::from_ref(&id))
                    .unwrap()
                    .status,
                SubmissionStatus::Prepared
            );
            assert_eq!(
                f.store.lock().unwrap().index.records[&id]
                    .submission_id
                    .as_deref(),
                Some(new_key.as_str())
            );
            assert!(f.store.status(&f.owner, &id).unwrap().expires_at >= now() + RETAINED_TTL - 1);
            f.store.begin_dispatch(&f.owner, &new_key).unwrap();
        }
    }
    #[test]
    fn reclaim_before_proof_cleanup_does_not_double_charge_retained_bytes() {
        let f = Fixture::new();
        let id = f.upload();
        let prior = f.prepare(&id);
        f.store.begin_dispatch(&f.owner, &prior).unwrap();
        f.store
            .finish_submission(&f.owner, &prior, DispatchOutcome::SubmittedUnconfirmed)
            .unwrap();
        let old = expire_terminal_claim(&f, &prior);
        let other = f.upload();
        {
            let mut state = f.store.lock().unwrap();
            let mut next = state.index.clone();
            let other_record = next.records.get_mut(&other).unwrap();
            other_record.metadata.size_bytes = RETAINED_BYTES - 5;
            other_record.submission_id = Some("already-pruned-terminal-proof".into());
            f.store.update(&mut state, next).unwrap();
        }
        let mut foreign = f.owner.clone();
        foreign.device_id.push('x');
        assert_eq!(
            f.store
                .prepare_submission(&foreign, &submission_key(), "", std::slice::from_ref(&id))
                .unwrap_err(),
            AttachmentError::NotFound
        );
        assert!(f
            .store
            .prepare_submission(&f.owner, &submission_key(), "", std::slice::from_ref(&id))
            .is_ok());
        assert_eq!(
            f.store.submission_status(&f.owner, &old).unwrap_err(),
            AttachmentError::SubmissionExpired
        );
    }
    #[test]
    fn failed_reclaim_and_expired_prepared_restore_the_prior_retention_promise() {
        let f = Fixture::new();
        let id = f.upload();
        let prior = f.prepare(&id);
        f.store.begin_dispatch(&f.owner, &prior).unwrap();
        f.store
            .finish_submission(&f.owner, &prior, DispatchOutcome::Submitted)
            .unwrap();
        let old = expire_terminal_claim(&f, &prior);
        let expiry = f.store.status(&f.owner, &id).unwrap().expires_at;
        let fresh = submission_key();
        f.store
            .prepare_submission(&f.owner, &fresh, "new", std::slice::from_ref(&id))
            .unwrap();
        f.store
            .finish_submission(&f.owner, &fresh, DispatchOutcome::FailedBeforeDispatch)
            .unwrap();
        {
            let state = f.store.lock().unwrap();
            let record = &state.index.records[&id];
            assert_eq!(record.submission_id.as_deref(), Some(old.as_str()));
            assert_eq!(record.metadata.expires_at, expiry);
            assert!(!counts_as_pending(&state.index, record));
            assert!(record.rollback_claim.is_none());
        }
        let fresh = submission_key();
        f.store
            .prepare_submission(&f.owner, &fresh, "new again", std::slice::from_ref(&id))
            .unwrap();
        {
            let mut state = f.store.lock().unwrap();
            let mut next = state.index.clone();
            next.submissions.get_mut(&fresh).unwrap().expires_at_ms = now_ms().saturating_sub(2000);
            f.store.update(&mut state, next).unwrap();
        }
        f.store.cleanup().unwrap();
        let state = f.store.lock().unwrap();
        let record = &state.index.records[&id];
        assert_eq!(record.submission_id.as_deref(), Some(old.as_str()));
        assert_eq!(record.metadata.expires_at, expiry);
        assert!(!counts_as_pending(&state.index, record));
        assert!(record_path(record).is_file());
    }
    #[test]
    fn expired_dispatching_claims_cannot_be_reclaimed_under_new_keys() {
        let f = Fixture::new();
        let id = f.upload();
        let key = f.prepare(&id);
        f.store.begin_dispatch(&f.owner, &key).unwrap();
        {
            let mut state = f.store.lock().unwrap();
            let mut next = state.index.clone();
            next.submissions.get_mut(&key).unwrap().expires_at_ms = now_ms().saturating_sub(2000);
            f.store.update(&mut state, next).unwrap();
        }
        f.store.cleanup().unwrap();
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &submission_key(), "explicit new intent", &[id])
                .unwrap_err(),
            AttachmentError::Conflict
        );
    }
    #[test]
    fn repeated_zero_byte_failed_submissions_stop_at_the_proof_count_cap() {
        let f = Fixture::new();
        let upload = Uuid::new_v4().to_string();
        f.reserve(&upload)
            .complete("empty.txt", "text/plain", b"")
            .unwrap();
        let first = f.prepare(&upload);
        f.store
            .finish_submission(&f.owner, &first, DispatchOutcome::FailedBeforeDispatch)
            .unwrap();
        // Seed compact proofs near the real production cap to avoid thousands of
        // filesystem syncs; the final repeated zero-byte attempts use the real API.
        {
            let mut state = f.store.lock().unwrap();
            let mut next = state.index.clone();
            let proof = next.submissions[&first].clone();
            while next.submissions.len() < MAX_SUBMISSIONS - 3 {
                next.submissions.insert(submission_key(), proof.clone());
            }
            f.store.update(&mut state, next).unwrap();
        }
        for _ in 0..3 {
            let key = f.prepare(&upload);
            f.store
                .finish_submission(&f.owner, &key, DispatchOutcome::FailedBeforeDispatch)
                .unwrap();
        }
        let rejected = submission_key();
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &rejected, "", std::slice::from_ref(&upload))
                .unwrap_err(),
            AttachmentError::QuotaExceeded
        );
        let state = f.store.lock().unwrap();
        assert_eq!(state.index.submissions.len(), MAX_SUBMISSIONS);
        assert!(!state.index.submissions.contains_key(&rejected));
        assert!(state.index.records[&upload].submission_id.is_none());
    }
    #[test]
    fn workspace_uploads_are_git_ignored_and_startup_restores_the_marker() {
        let mut f = Fixture::new();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&f.workspace)
            .status()
            .unwrap()
            .success());
        let project_ignore = f.workspace.join(".gitignore");
        fs::write(&project_ignore, "existing-project-rule\n").unwrap();
        let id = f.upload();
        let path = record_path(&f.store.lock().unwrap().index.records[&id]);
        assert!(std::process::Command::new("git")
            .args(["check-ignore", "--quiet", "--"])
            .arg(
                path.strip_prefix(f.workspace.canonicalize().unwrap())
                    .unwrap()
            )
            .current_dir(&f.workspace)
            .status()
            .unwrap()
            .success());
        assert_eq!(
            fs::read_to_string(&project_ignore).unwrap(),
            "existing-project-rule\n"
        );
        let marker = f.workspace.join(".wardian/attachments/remote/.gitignore");
        assert_eq!(fs::read(&marker).unwrap(), b"*\n");
        fs::remove_file(&marker).unwrap();
        drop(f.store);
        f.store = AttachmentStore::open(&f.home).unwrap();
        assert_eq!(fs::read(&marker).unwrap(), b"*\n");
        assert!(f.store.status(&f.owner, &id).is_ok());
    }

    #[test]
    fn foreign_upload_ignore_rules_fail_closed_without_overwriting_them() {
        let f = Fixture::new();
        drop(f.reserve(&Uuid::new_v4().to_string()));
        let marker = f.workspace.join(".wardian/attachments/remote/.gitignore");
        let foreign = b"*.txt\n!*.png\n";
        fs::write(&marker, foreign).unwrap();
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::DiskFailure)
        ));
        assert_eq!(fs::read(&marker).unwrap(), foreign);
        assert!(f.store.lock().unwrap().index.records.is_empty());
    }

    #[test]
    fn serialized_index_byte_cap_rejects_before_claims_without_poisoning_store() {
        let f = Fixture::new();
        let upload = f.upload();
        let key = submission_key();
        // JSON escaping, the generated references and metadata count too.
        let prompt = format!("nonempty{}", "\n".repeat(MAX_INDEX_BYTES as usize / 2));
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &key, &prompt, std::slice::from_ref(&upload))
                .unwrap_err(),
            AttachmentError::QuotaExceeded
        );
        {
            let state = f.store.lock().unwrap();
            assert!(state.index.submissions.is_empty());
            assert!(state.index.records[&upload].submission_id.is_none());
        }
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &key, "fits", &[upload])
                .unwrap()
                .status,
            SubmissionStatus::Prepared
        );
    }
    #[test]
    fn metadata_quota_rejects_uploads_before_creating_or_publishing_blobs() {
        let f = Fixture::new();
        let retained_upload = f.upload();
        let key = f.prepare(&retained_upload);
        {
            let mut state = f.store.lock().unwrap();
            let mut next = state.index.clone();
            next.submissions.get_mut(&key).unwrap().resolved_prompt = Some(String::new());
            let base = serde_json::to_vec(&next).unwrap().len();
            next.submissions.get_mut(&key).unwrap().resolved_prompt =
                Some("p".repeat(MAX_INDEX_BYTES as usize - base - 1));
            assert_eq!(
                serde_json::to_vec(&next).unwrap().len() as u64,
                MAX_INDEX_BYTES - 1
            );
            f.store.update(&mut state, next).unwrap();
        }
        let root = f.workspace.join(".wardian/attachments/remote");
        for _ in 0..2 {
            let id = Uuid::new_v4().to_string();
            let guard = f.reserve(&id);
            assert_eq!(
                guard
                    .complete("rejected.txt", "text/plain", b"must not become an orphan")
                    .unwrap_err(),
                AttachmentError::QuotaExceeded
            );
            assert!(!root.join(&id).exists());
            assert!(!f.store.lock().unwrap().index.records.contains_key(&id));
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2); // Uploaded directory plus the private ignore marker.
        assert!(f.store.status(&f.owner, &retained_upload).is_ok());
    }
    #[test]
    fn startup_rejects_oversized_index_before_deserialization() {
        let f = Fixture::new();
        let index_path = f.store.root.join("index.json");
        let home = f.home.clone();
        drop(f.store);
        fs::write(index_path, vec![b' '; MAX_INDEX_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            AttachmentStore::open(&home),
            Err(AttachmentError::QuotaExceeded)
        ));
    }
    #[test]
    fn workspace_paths_and_atomic_publication() {
        let f = Fixture::new();
        let id = f.upload();
        let path = f
            .workspace
            .join(".wardian/attachments/remote")
            .join(&id)
            .join(format!("{id}.txt"));
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert!(!f.home.join("remote/attachments").join(&id).exists());
        let dto = serde_json::to_string(&f.store.status(&f.owner, &id).unwrap()).unwrap();
        assert!(!dto.contains("workspace"));
        assert!(!dto.contains("digest"));
    }
    #[test]
    fn durable_identical_retry_and_conflict() {
        let mut f = Fixture::new();
        let id = f.upload();
        let old = f.store.status(&f.owner, &id).unwrap();
        assert_eq!(
            f.reserve(&id)
                .complete("notes.txt", "text/plain", b"hello")
                .unwrap(),
            old
        );
        assert_eq!(
            f.reserve(&id)
                .complete("notes.txt", "text/plain", b"changed")
                .unwrap_err(),
            AttachmentError::Conflict
        );
        assert_eq!(
            f.reserve(&id)
                .complete("other.txt", "text/plain", b"hello")
                .unwrap_err(),
            AttachmentError::Conflict
        );
        drop(f.store);
        f.store = AttachmentStore::open(&f.home).unwrap();
        assert_eq!(f.store.status(&f.owner, &id).unwrap(), old);
        assert_eq!(
            f.reserve(&id)
                .complete("notes.txt", "text/plain", b"hello")
                .unwrap(),
            old
        );
    }
    #[test]
    fn each_owner_dimension_is_enforced() {
        let f = Fixture::new();
        let id = f.upload();
        let sid = f.prepare(&id);
        for changed in 0..3 {
            let mut owner = f.owner.clone();
            match changed {
                0 => owner.device_id.push('x'),
                1 => owner.origin.push('x'),
                _ => owner.agent_id.push('x'),
            }
            assert_eq!(
                f.store.status(&owner, &id).unwrap_err(),
                AttachmentError::NotFound
            );
            assert!(matches!(
                f.store.reserve(&owner, &f.workspace, &id),
                Err(AttachmentError::NotFound)
            ));
            assert_eq!(
                f.store.submission_status(&owner, &sid).unwrap_err(),
                AttachmentError::NotFound
            );
            assert_eq!(
                f.store
                    .submission_status(&owner, &submission_key())
                    .unwrap_err(),
                AttachmentError::NotFound
            );
            f.store.delete(&owner, &id).unwrap();
            assert!(f.store.status(&f.owner, &id).is_ok());
        }
    }
    #[test]
    fn cancelled_and_invalid_uploads_release_capacity() {
        let f = Fixture::new();
        let id = Uuid::new_v4().to_string();
        let one = f.reserve(&id);
        let two = f.reserve(&Uuid::new_v4().to_string());
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        assert!(matches!(
            f.store.reserve(&f.owner, &f.workspace, &id),
            Err(AttachmentError::Busy)
        ));
        drop(one);
        drop(two);
        assert_eq!(
            f.reserve(&id)
                .complete("evil.exe", "text/plain", b"x")
                .unwrap_err(),
            AttachmentError::UnsupportedType
        );
        assert!(f
            .reserve(&id)
            .complete("ok.txt", "text/plain", b"ok")
            .is_ok());
    }
    #[test]
    fn revoked_guard_cannot_publish_or_cancel_its_replacement() {
        let f = Fixture::new();
        let id = Uuid::new_v4().to_string();
        let old = f.reserve(&id);
        f.store.revoke_device(&f.owner.device_id).unwrap();
        assert!(matches!(
            f.store.reserve(&f.owner, &f.workspace, &id),
            Err(AttachmentError::Busy)
        ));
        assert_eq!(
            old.complete("old.txt", "text/plain", b"bad").unwrap_err(),
            AttachmentError::NotFound
        );
        let new = f.reserve(&id);
        assert!(new.complete("new.txt", "text/plain", b"ok").is_ok());
    }
    #[test]
    fn delete_cancels_an_inflight_upload() {
        let f = Fixture::new();
        let id = Uuid::new_v4().to_string();
        let guard = f.reserve(&id);
        let mut foreign = f.owner.clone();
        foreign.device_id.push('x');
        f.store.delete(&foreign, &id).unwrap();
        assert!(f.store.lock().unwrap().reservations.contains_key(&id));
        f.store.delete(&f.owner, &id).unwrap();
        assert_eq!(
            guard
                .complete("notes.txt", "text/plain", b"late")
                .unwrap_err(),
            AttachmentError::NotFound
        );
    }
    #[test]
    fn cancelled_readers_remain_charged_until_guard_drop() {
        let f = Fixture::new();
        let a = Uuid::new_v4().to_string();
        let b = Uuid::new_v4().to_string();
        let first = f.reserve(&a);
        let second = f.reserve(&b);
        f.store.delete(&f.owner, &a).unwrap();
        f.store.delete(&f.owner, &b).unwrap();
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        drop(first);
        assert!(f
            .store
            .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string())
            .is_ok());
        drop(second);
    }
    #[test]
    fn prepared_files_keep_pending_rollback_capacity() {
        let f = Fixture::new();
        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(f.upload());
        }
        let sid = f.prepare(&ids[0]);
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        f.store.begin_dispatch(&f.owner, &sid).unwrap();
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        f.store
            .finish_submission(&f.owner, &sid, DispatchOutcome::FailedBeforeDispatch)
            .unwrap();
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        let sid = f.prepare(&ids[0]);
        f.store.begin_dispatch(&f.owner, &sid).unwrap();
        f.store
            .finish_submission(&f.owner, &sid, DispatchOutcome::Submitted)
            .unwrap();
        assert!(f
            .store
            .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string())
            .is_ok());
    }
    #[test]
    fn animated_images_are_bounded_and_fully_validated() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for _ in 0..2 {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::new(2, 2)))
                    .unwrap();
            }
        }
        assert!(validate("a.gif", "image/gif", &bytes).is_ok());
        // A valid first frame must not conceal a truncated later frame.
        let cut = bytes.len() - 6;
        assert_eq!(
            validate("a.gif", "image/gif", &bytes[..cut]),
            Err(AttachmentError::InvalidImage)
        );
        let frame = image::Frame::new(image::RgbaImage::new(1, 1));
        let frames = image::Frames::new(Box::new((0..1001).map(move |_| Ok(frame.clone()))));
        assert_eq!(validate_frames(frames), Err(AttachmentError::InvalidImage));
    }
    #[test]
    fn pending_count_and_global_reservations() {
        let f = Fixture::new();
        for _ in 0..8 {
            f.upload();
        }
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        let g = Fixture::new();
        let mut reservations = Vec::new();
        for i in 0..20 {
            let mut owner = g.owner.clone();
            owner.device_id = i.to_string();
            reservations.push(
                g.store
                    .reserve(&owner, &g.workspace, &Uuid::new_v4().to_string())
                    .unwrap(),
            );
        }
        let mut owner = g.owner.clone();
        owner.device_id = "extra".into();
        assert!(matches!(
            g.store
                .reserve(&owner, &g.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        drop(reservations);
        assert!(g
            .store
            .reserve(&owner, &g.workspace, &Uuid::new_v4().to_string())
            .is_ok());
    }
    #[test]
    fn retry_bodies_also_reserve_global_byte_capacity() {
        let f = Fixture::new();
        let id = f.upload();
        let mut guards = Vec::new();
        for i in 0..19 {
            let mut owner = f.owner.clone();
            owner.device_id = format!("other-{i}");
            guards.push(
                f.store
                    .reserve(&owner, &f.workspace, &Uuid::new_v4().to_string())
                    .unwrap(),
            );
        }
        // 190 MiB in live readers plus the pending file means another full-size
        // reader exceeds 200 MiB, even when its upload ID already exists.
        assert!(matches!(
            f.store.reserve(&f.owner, &f.workspace, &id),
            Err(AttachmentError::QuotaExceeded)
        ));
        drop(guards);
        assert!(f.store.reserve(&f.owner, &f.workspace, &id).is_ok());
    }
    #[test]
    fn concurrency_reserves_before_body() {
        let f = Fixture::new();
        let barrier = Arc::new(std::sync::Barrier::new(9));
        let guards = std::thread::scope(|scope| {
            let mut jobs = Vec::new();
            for _ in 0..8 {
                let b = barrier.clone();
                let f = &f;
                jobs.push(scope.spawn(move || {
                    b.wait();
                    f.store
                        .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string())
                }));
            }
            barrier.wait();
            jobs.into_iter()
                .filter_map(|j| j.join().unwrap().ok())
                .collect::<Vec<_>>()
        });
        assert_eq!(guards.len(), 2);
    }
    #[test]
    fn filename_types_and_text_validation() {
        for name in [
            "../x.txt",
            "a/b.txt",
            "a\\b.txt",
            "x:stream.txt",
            "..txt",
            "a\n.txt",
            "x.txt ",
            "a..txt",
            ".txt",
        ] {
            assert_eq!(
                validate(name, "text/plain", b"ok"),
                Err(AttachmentError::InvalidFilename),
                "{name:?}"
            );
        }
        assert_eq!(
            validate(&format!("{}.txt", "a".repeat(125)), "text/plain", b"x"),
            Err(AttachmentError::InvalidFilename)
        );
        for name in ["a.html", "a.svg", "a.zip", "a.exe", "a.heic"] {
            assert_eq!(
                validate(name, "text/plain", b"x"),
                Err(AttachmentError::UnsupportedType)
            );
        }
        assert_eq!(
            validate("a.png", "text/plain", b"x"),
            Err(AttachmentError::UnsupportedType)
        );
        assert_eq!(
            validate("a.pdf", "application/pdf", b"%PDF-1.7\n"),
            Ok("pdf".into())
        );
        assert_eq!(
            validate("a.pdf", "application/pdf", b"bad"),
            Err(AttachmentError::InvalidContent)
        );
        assert!(validate("a.md", "text/markdown", b"# hi\n").is_ok());
        assert!(validate("a.csv", "text/csv", b"a,b\n1,2").is_ok());
        assert!(validate("a.json", "application/json", b"{\"a\":1}").is_ok());
        assert_eq!(
            validate("a.json", "application/json", b"{"),
            Err(AttachmentError::InvalidContent)
        );
        assert_eq!(
            validate("a.txt", "text/plain", &[255]),
            Err(AttachmentError::InvalidContent)
        );
        assert_eq!(
            validate("a.txt", "text/plain", b"\x1b[31m"),
            Err(AttachmentError::InvalidContent)
        );
    }
    fn image_bytes(format: image::ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }
    #[test]
    fn image_decoding_signatures_and_bombs() {
        use image::ImageFormat;
        for (ext, mime, format) in [
            ("png", "image/png", ImageFormat::Png),
            ("jpg", "image/jpeg", ImageFormat::Jpeg),
            ("webp", "image/webp", ImageFormat::WebP),
            ("gif", "image/gif", ImageFormat::Gif),
            ("bmp", "image/bmp", ImageFormat::Bmp),
        ] {
            let bytes = image_bytes(format);
            assert!(validate(&format!("x.{ext}"), mime, &bytes).is_ok());
            assert_eq!(
                validate(&format!("x.{ext}"), mime, &bytes[..bytes.len() / 2]),
                Err(AttachmentError::InvalidImage)
            );
        }
        let mut bomb = image_bytes(ImageFormat::Bmp);
        bomb[18..22].copy_from_slice(&100_000i32.to_le_bytes());
        bomb[22..26].copy_from_slice(&100_000i32.to_le_bytes());
        assert_eq!(
            validate("bomb.bmp", "image/bmp", &bomb),
            Err(AttachmentError::InvalidImage)
        );
        assert_eq!(
            validate("fake.png", "image/png", &image_bytes(ImageFormat::Jpeg)),
            Err(AttachmentError::InvalidImage)
        );
    }
    #[test]
    fn file_limit_enforced_by_completion() {
        let f = Fixture::new();
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            f.reserve(&id)
                .complete(
                    "a.txt",
                    "text/plain",
                    &vec![b'x'; MAX_FILE_BYTES as usize + 1]
                )
                .unwrap_err(),
            AttachmentError::FileTooLarge
        );
        assert!(f
            .reserve(&id)
            .complete("a.txt", "text/plain", &vec![b'x'; MAX_FILE_BYTES as usize])
            .is_ok());
    }
    #[test]
    fn message_and_retained_admission_are_atomic_at_the_byte_limits() {
        let f = Fixture::new();
        let bytes = vec![b'x'; MAX_FILE_BYTES as usize];
        let mut ids = Vec::new();
        for _ in 0..3 {
            let id = Uuid::new_v4().to_string();
            f.reserve(&id)
                .complete("large.txt", "text/plain", &bytes)
                .unwrap();
            ids.push(id);
        }
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &submission_key(), "", &ids)
                .unwrap_err(),
            AttachmentError::QuotaExceeded
        );
        assert!(f
            .store
            .lock()
            .unwrap()
            .index
            .records
            .values()
            .all(|r| r.submission_id.is_none()));
        f.prepare(&ids[0]);
        // Exercise retained-ledger admission without writing a gigabyte fixture.
        {
            let mut state = f.store.lock().unwrap();
            state
                .index
                .records
                .get_mut(&ids[0])
                .unwrap()
                .metadata
                .size_bytes = RETAINED_BYTES;
        }
        assert_eq!(
            f.store
                .prepare_submission(
                    &f.owner,
                    &submission_key(),
                    "",
                    std::slice::from_ref(&ids[1])
                )
                .unwrap_err(),
            AttachmentError::QuotaExceeded
        );
        assert!(f.store.lock().unwrap().index.records[&ids[1]]
            .submission_id
            .is_none());
    }
    #[test]
    fn device_byte_quota_includes_outstanding_reservations() {
        let f = Fixture::new();
        let bytes = vec![b'x'; MAX_FILE_BYTES as usize];
        for _ in 0..4 {
            f.reserve(&Uuid::new_v4().to_string())
                .complete("large.txt", "text/plain", &bytes)
                .unwrap();
        }
        let guard = f.reserve(&Uuid::new_v4().to_string());
        assert!(matches!(
            f.store
                .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::QuotaExceeded)
        ));
        drop(guard);
        assert!(f
            .store
            .reserve(&f.owner, &f.workspace, &Uuid::new_v4().to_string())
            .is_ok());
    }
    #[cfg(windows)]
    #[test]
    fn provider_references_strip_verbatim_prefix_only_for_display() {
        let drive = PathBuf::from(r"\\?\C:\workspace\.wardian\attachments\remote\file.txt");
        assert_eq!(
            provider_reference(&drive),
            r"C:\workspace\.wardian\attachments\remote\file.txt"
        );
        assert!(drive.to_str().unwrap().starts_with(r"\\?\"));
        assert_eq!(
            provider_reference(Path::new(r"\\?\UNC\server\share\file.png")),
            r"\\server\share\file.png"
        );
        assert_eq!(
            provider_reference(Path::new(r"C:\ordinary\file.txt")),
            r"C:\ordinary\file.txt"
        );
        assert_eq!(
            provider_reference(Path::new(r"\\server\share\file.txt")),
            r"\\server\share\file.txt"
        );
    }
    #[test]
    fn path_only_prompt_and_submission_payload_identity() {
        let f = Fixture::new();
        let a = f.upload();
        let b = Uuid::new_v4().to_string();
        f.reserve(&b)
            .complete(
                "photo.png",
                "image/png",
                &image_bytes(image::ImageFormat::Png),
            )
            .unwrap();
        let sid = submission_key();
        let ids = [a.clone(), b.clone()];
        let p = f
            .store
            .prepare_submission(&f.owner, &sid, "", &ids)
            .unwrap();
        assert!(p
            .prompt
            .starts_with("Please inspect the attached files.\n\nAttached files:\n- "));
        assert_eq!(
            p.prompt
                .lines()
                .filter(|line| line.starts_with("- "))
                .count(),
            2
        );
        assert!(p
            .prompt
            .lines()
            .filter_map(|line| line.strip_prefix("- "))
            .all(|reference| Path::new(reference).is_absolute()));
        assert!(p.prompt.ends_with(&format!("{b}.png")));
        #[cfg(windows)]
        {
            assert!(!p.prompt.contains(r"\\?\"));
            let state = f.store.lock().unwrap();
            assert!(state.index.records[&a]
                .workspace
                .to_str()
                .unwrap()
                .starts_with(r"\\?\"));
        }
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &sid, "", &ids)
                .unwrap()
                .prompt,
            p.prompt
        );
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &sid, "changed", &ids)
                .unwrap_err(),
            AttachmentError::Conflict
        );
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &sid, "", &[b, a])
                .unwrap_err(),
            AttachmentError::Conflict
        );
        assert_eq!(
            f.store.delete(&f.owner, &ids[0]).unwrap_err(),
            AttachmentError::Conflict
        );
    }
    #[test]
    fn all_or_nothing_claims_and_unique_ids() {
        let f = Fixture::new();
        let id = f.upload();
        assert_eq!(
            f.store
                .prepare_submission(
                    &f.owner,
                    &submission_key(),
                    "x",
                    &[id.clone(), Uuid::new_v4().to_string()]
                )
                .unwrap_err(),
            AttachmentError::NotFound
        );
        assert!(f.store.lock().unwrap().index.records[&id]
            .submission_id
            .is_none());
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &submission_key(), "x", &[id.clone(), id.clone()])
                .unwrap_err(),
            AttachmentError::InvalidContent
        );
        f.prepare(&id);
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &submission_key(), "x", &[id])
                .unwrap_err(),
            AttachmentError::Conflict
        );
    }
    #[test]
    fn durable_fence_restart_and_status() {
        let mut f = Fixture::new();
        let id = f.upload();
        let sid = f.prepare(&id);
        assert_eq!(
            f.store.begin_dispatch(&f.owner, &sid).unwrap(),
            SubmissionStatus::Dispatching
        );
        assert_eq!(
            f.store.begin_dispatch(&f.owner, &sid).unwrap_err(),
            AttachmentError::DispatchFenced
        );
        drop(f.store);
        f.store = AttachmentStore::open(&f.home).unwrap();
        assert_eq!(
            f.store.submission_status(&f.owner, &sid).unwrap(),
            SubmissionStatus::SubmittedUnconfirmed
        );
        assert_eq!(
            f.store.begin_dispatch(&f.owner, &sid).unwrap_err(),
            AttachmentError::DispatchFenced
        );
        assert_eq!(
            f.store
                .prepare_submission(&f.owner, &sid, "", &[id])
                .unwrap()
                .status,
            SubmissionStatus::SubmittedUnconfirmed
        );
    }
    #[test]
    fn finish_and_proven_failure_release_claims_without_extending_pending_ttl() {
        let f = Fixture::new();
        let id = f.upload();
        let expires = f.store.status(&f.owner, &id).unwrap().expires_at;
        let sid = f.prepare(&id);
        f.store.begin_dispatch(&f.owner, &sid).unwrap();
        f.store
            .finish_submission(&f.owner, &sid, DispatchOutcome::FailedBeforeDispatch)
            .unwrap();
        assert_eq!(f.store.status(&f.owner, &id).unwrap().expires_at, expires);
        assert_eq!(
            f.store.begin_dispatch(&f.owner, &sid).unwrap_err(),
            AttachmentError::DispatchFenced
        );
        let new = f.prepare(&id);
        f.store.begin_dispatch(&f.owner, &new).unwrap();
        f.store
            .finish_submission(&f.owner, &new, DispatchOutcome::Submitted)
            .unwrap();
        assert_eq!(
            f.store
                .finish_submission(&f.owner, &new, DispatchOutcome::Submitted)
                .unwrap(),
            SubmissionStatus::Submitted
        );
    }
    #[test]
    fn expiry_cleanup_retention_and_active_pin() {
        let f = Fixture::new();
        let pending = f.upload();
        let retained = f.upload();
        let sid = f.prepare(&retained);
        f.store.cleanup_at(now() + PENDING_TTL + 1).unwrap();
        assert_eq!(
            f.store.status(&f.owner, &pending).unwrap_err(),
            AttachmentError::NotFound
        );
        assert!(f.store.status(&f.owner, &retained).is_ok());
        f.store.begin_dispatch(&f.owner, &sid).unwrap();
        f.store.cleanup_at(now() + RETAINED_TTL + 1).unwrap();
        assert!(f.store.status(&f.owner, &retained).is_ok());
        f.store
            .finish_submission(&f.owner, &sid, DispatchOutcome::Submitted)
            .unwrap();
        f.store.cleanup_at(now() + RETAINED_TTL + 1).unwrap();
        assert_eq!(
            f.store.status(&f.owner, &retained).unwrap_err(),
            AttachmentError::NotFound
        );
        assert_eq!(
            f.store.submission_status(&f.owner, &sid).unwrap_err(),
            AttachmentError::NotFound
        );
    }
    #[test]
    fn startup_cleans_crash_orphans_but_preserves_unrelated_files() {
        let mut f = Fixture::new();
        let id = f.upload();
        let root = f.workspace.join(".wardian/attachments/remote");
        let orphan = root.join(Uuid::new_v4().to_string());
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("temporary"), b"orphan").unwrap();
        fs::write(root.join("unrelated"), b"keep").unwrap();
        let index_temp = f.store.root.join(".attachment-index-crash");
        fs::write(&index_temp, b"incomplete index").unwrap();
        drop(f.store);
        f.store = AttachmentStore::open(&f.home).unwrap();
        assert!(!orphan.exists());
        assert!(!index_temp.exists());
        assert!(root.join("unrelated").exists());
        assert!(f.store.status(&f.owner, &id).is_ok());
    }
    #[test]
    fn second_opener_and_malformed_ids_are_rejected() {
        let f = Fixture::new();
        assert!(matches!(
            AttachmentStore::open(&f.home),
            Err(AttachmentError::Busy)
        ));
        for id in [
            "../escape",
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-4000-0000-000000000000",
            "38C8D996-420A-49EE-AB7B-B34D566EB700",
        ] {
            assert!(matches!(
                f.store.reserve(&f.owner, &f.workspace, id),
                Err(AttachmentError::InvalidId)
            ));
        }
    }
    #[test]
    fn storage_failure_is_sanitized_and_does_not_publish() {
        let f = Fixture::new();
        let id = Uuid::new_v4().to_string();
        let guard = f.reserve(&id);
        fs::create_dir(f.store.root.join("index.json.blocker")).unwrap();
        let index = f.store.root.join("index.json");
        fs::remove_file(&index).unwrap();
        fs::create_dir(&index).unwrap();
        let error = guard
            .complete("notes.txt", "text/plain", b"ok")
            .unwrap_err();
        assert_eq!(error.to_string(), "attachment_storage_failure");
        assert_eq!(
            f.store.status(&f.owner, &id).unwrap_err(),
            AttachmentError::DiskFailure
        );
    }
    #[cfg(windows)]
    #[test]
    fn cleanup_and_reserve_refuse_junction_escapes() {
        let f = Fixture::new();
        let id = f.upload();
        let root = f.workspace.join(".wardian/attachments/remote");
        let outside = f._temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"safe").unwrap();
        let link = root.join(Uuid::new_v4().to_string());
        junction::create(&outside, &link).unwrap();
        assert_eq!(f.store.cleanup(), Err(AttachmentError::DiskFailure));
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"safe");
        assert!(f.store.status(&f.owner, &id).is_ok());
        junction::delete(&link).unwrap();
        let linked_workspace = f._temp.path().join("linked-workspace");
        junction::create(&outside, &linked_workspace).unwrap();
        assert!(matches!(
            f.store
                .reserve(&f.owner, &linked_workspace, &Uuid::new_v4().to_string()),
            Err(AttachmentError::DiskFailure)
        ));
        junction::delete(&linked_workspace).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn cleanup_refuses_symlink_escape() {
        let f = Fixture::new();
        let id = f.upload();
        let root = f.workspace.join(".wardian/attachments/remote");
        let outside = f._temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"safe").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(Uuid::new_v4().to_string())).unwrap();
        assert_eq!(f.store.cleanup(), Err(AttachmentError::DiskFailure));
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"safe");
        assert!(f.store.status(&f.owner, &id).is_ok());
    }
}
