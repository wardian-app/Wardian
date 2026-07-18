//! Backend-owned file subscriptions, stable revisions, and bounded read leases.

use notify::Watcher as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt as _};
use tauri::{Emitter as _, Manager as _};
use tokio::sync::{broadcast, Mutex, OwnedMutexGuard};
use uuid::Uuid;
use wardian_core::files::{
    AuthorizedPath, AuthorizedRootService, FileContentDescriptorV1, FileRendererKind,
    FileResourceErrorV1, FileResourceLimits, FileRevisionToken, VerifiedFileSnapshot,
};
use wardian_core::models::AgentConfig;

pub const FILE_RESOURCE_REVISION_EVENT: &str = "file-resource://revision";
const DEFAULT_STABILITY_DELAY: Duration = Duration::from_millis(150);
const DEFAULT_TICKET_TTL: Duration = Duration::from_secs(60);
const DEFAULT_MAX_USER_FILE_GRANTS: usize = 128;
const DEFAULT_MAX_SAVE_TARGET_GRANTS: usize = 32;
const DEFAULT_SAVE_TARGET_TTL: Duration = Duration::from_secs(60);
const MAX_TICKET_SNAPSHOT_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_TICKET_SNAPSHOT_RESERVATION_BYTES: u64 = 4 * 1024 * 1024;
const RECOVERY_ORPHAN_GRACE_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_MAX_RECOVERY_RECORDS: usize = 128;
const DEFAULT_MAX_RECOVERY_BODY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileResourceSnapshotV1 {
    pub resource_id: String,
    pub subscription_id: String,
    pub revision: u64,
    pub descriptor: FileContentDescriptorV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileResourceEventV1 {
    pub schema: u8,
    pub resource_id: String,
    pub revision: u64,
    pub descriptor: FileContentDescriptorV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileResourceTextV1 {
    pub schema: u8,
    pub resource_id: String,
    pub revision: u64,
    pub text: String,
}

/// Tagged optimistic-save result returned to the frontend without exposing the
/// backend-private retained-handle revision token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FileResourceSaveResultV1 {
    /// The submitted text replaced the target and advanced the revision.
    Saved { revision: u64, content_hash: String },
    /// The submitted text was byte-identical to the current target.
    Unchanged { revision: u64, content_hash: String },
    /// The editor base no longer matches the currently authorized target.
    StaleConflict { revision: u64, content_hash: String },
}

/// Metadata returned after an editor recovery checkpoint is durably committed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileRecoveryCheckpointV1 {
    pub schema: u8,
    pub recovery_id: String,
    pub resource_key: String,
    pub base_content_hash: String,
    pub base_opaque_revision: String,
    pub recovery_revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// Advisory current-file authorization failure observed after the recovery
    /// bytes committed. This never gates or rolls back recovery durability.
    pub file_authorization_error: Option<FileResourceErrorV1>,
}

/// Body-free metadata used to discover durable editor recovery records after
/// a frontend or native runtime restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileRecoverySummaryV1 {
    pub schema: u8,
    pub recovery_id: String,
    pub resource_key: String,
    pub display_name: String,
    pub extension: Option<String>,
    pub mime_type: String,
    pub base_content_hash: String,
    pub base_opaque_revision: String,
    pub recovery_revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Optional exact durable-recovery generation cleaned after a successful
/// guarded save. The calling WebView scope is supplied by the command layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct FileRecoveryCleanupV1 {
    pub recovery_id: String,
    pub expected_recovery_revision: u64,
}

/// Read-only durable editor recovery payload. It contains only the persisted
/// base and buffer; current filesystem bytes require a live subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileRecoveryV1 {
    pub schema: u8,
    pub recovery_id: String,
    pub resource_key: String,
    pub display_name: String,
    pub extension: Option<String>,
    pub mime_type: String,
    pub base_content_hash: String,
    pub base_opaque_revision: String,
    pub recovery_revision: u64,
    pub base: String,
    pub buffer: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Structured three-way recovery merge outcome. Conflicts always return
/// explicit markers instead of selecting either editor or disk bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FileRecoveryMergeResultV1 {
    Clean {
        recovery_revision: u64,
        current_revision: u64,
        current_content_hash: String,
        disk_changed: bool,
        merged_text: String,
    },
    Conflicted {
        recovery_revision: u64,
        current_revision: u64,
        current_content_hash: String,
        disk_changed: bool,
        merged_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FileRecoveryManifestV1 {
    schema: u8,
    recovery_id: String,
    resource_key: String,
    display_name: String,
    extension: Option<String>,
    mime_type: String,
    base_content_hash: String,
    base_opaque_revision: String,
    base_blob: String,
    buffer_blob: String,
    recovery_revision: u64,
    webview_scope: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct FileRecoveryStoreLimits {
    max_records: usize,
    max_body_bytes: u64,
    orphan_grace_period: Duration,
}

impl Default for FileRecoveryStoreLimits {
    fn default() -> Self {
        Self {
            max_records: DEFAULT_MAX_RECOVERY_RECORDS,
            max_body_bytes: DEFAULT_MAX_RECOVERY_BODY_BYTES,
            orphan_grace_period: RECOVERY_ORPHAN_GRACE_PERIOD,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct FileRecoveryStoreUsage {
    records: usize,
    body_bytes: u64,
}

/// Short-lived one-shot authority returned by the native Save As picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SaveTargetGrantV1 {
    /// Response schema version.
    pub schema: u8,
    /// Opaque backend-owned grant identifier.
    pub save_target_grant_id: String,
    /// Selected path for display only; it is not filesystem authority.
    pub selected_path: String,
}

/// Ordinary exact-file capability created by a successful Save As operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileResourceSaveAsResultV1 {
    /// Response schema version.
    pub schema: u8,
    /// Opaque exact-file capability identifier for later opening.
    pub capability_id: String,
    /// Verified canonical path of the saved ordinary file.
    pub canonical_path: String,
    /// Stable `file:` resource identifier derived from the canonical path.
    pub resource_id: String,
    /// Hash of the durably written content.
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UserFileGrantV1 {
    pub schema: u8,
    pub capability_id: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileResourceTicketV1 {
    pub schema: u8,
    pub ticket_id: String,
    pub url: String,
    pub resource_id: String,
    pub revision: u64,
    pub renderer_lease_id: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileResourceRangeRead {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub start: u64,
    pub end: u64,
    pub total_size: u64,
    pub partial: bool,
}

#[derive(Clone)]
pub struct FileResourceRuntime {
    inner: Arc<FileResourceRuntimeInner>,
}

struct FileResourceRuntimeInner {
    entries: Mutex<HashMap<String, FileResourceEntry>>,
    subscription_resources: Mutex<HashMap<String, String>>,
    user_file_grants: Arc<Mutex<HashMap<String, UserFileGrant>>>,
    save_target_grants: Mutex<HashMap<String, SaveTargetGrant>>,
    read_tickets: Mutex<HashMap<String, FileReadTicket>>,
    renderer_leases: Mutex<HashMap<RendererLeaseKey, RendererLease>>,
    ticket_publication: Mutex<()>,
    limits: FileResourceLimits,
    stability_delay: Duration,
    ticket_ttl: Duration,
    max_user_file_grants: usize,
    max_save_target_grants: usize,
    save_target_ttl: Duration,
    ticket_snapshot_usage: Arc<AtomicU64>,
    max_ticket_snapshot_bytes: u64,
    events: broadcast::Sender<FileResourceEventV1>,
    app_handle: RwLock<Option<tauri::AppHandle>>,
    agent_config_resolver: RwLock<CurrentAgentConfigResolver>,
    recovery_root: RwLock<Option<PathBuf>>,
    recovery_io: Mutex<()>,
    recovery_store_limits: RwLock<FileRecoveryStoreLimits>,
    #[cfg(test)]
    issue_ticket_after_validation_hook: Mutex<Option<IssueTicketAfterValidationHook>>,
    #[cfg(test)]
    ticket_publication_hook: Mutex<Option<TicketPublicationHook>>,
    #[cfg(test)]
    forced_refresh_error: Mutex<Option<FileResourceErrorV1>>,
    #[cfg(test)]
    open_after_entry_miss_hook: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    #[cfg(test)]
    grant_eviction_before_lock_hook: Mutex<Option<GrantEvictionBeforeLockHook>>,
    #[cfg(test)]
    save_after_validation_hook: Mutex<Option<SaveAfterValidationHook>>,
    #[cfg(test)]
    fail_recovery_before_manifest: AtomicBool,
    #[cfg(test)]
    refresh_scan_count: AtomicU64,
}

#[cfg(test)]
#[derive(Clone)]
struct IssueTicketAfterValidationHook {
    validation_reached: Arc<tokio::sync::Barrier>,
    resume_publication: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
#[derive(Clone)]
struct TicketPublicationHook {
    pause_once: Arc<AtomicBool>,
    lease_published: Arc<tokio::sync::Barrier>,
    resume_publication: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
#[derive(Clone)]
struct GrantEvictionBeforeLockHook {
    reached: Arc<tokio::sync::Barrier>,
    resume: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
#[derive(Clone)]
struct SaveAfterValidationHook {
    validation_reached: Arc<tokio::sync::Barrier>,
    resume_save: Arc<tokio::sync::Barrier>,
}

struct FileResourceEntry {
    _watcher: notify::RecommendedWatcher,
    revision_token: FileRevisionToken,
    descriptor: FileContentDescriptorV1,
    revision: u64,
    incarnation_id: Uuid,
    subscribers: HashMap<String, FileSubscriptionAccess>,
    debounce_generation: u64,
    operation: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct FileSubscriptionAccess {
    claim: FileAccessClaim,
    authorized: AuthorizedPath,
}

#[derive(Clone)]
struct FileRefreshCandidate {
    subscription_id: String,
    access: FileSubscriptionAccess,
}

#[derive(Clone)]
enum FileAccessClaim {
    Agent { agent_id: String },
    User { capability_id: String },
}

#[derive(Clone)]
enum CurrentAgentConfigResolver {
    OpeningSnapshots(Arc<StdMutex<HashMap<String, AgentConfig>>>),
    AppState(tauri::AppHandle),
}

impl Default for CurrentAgentConfigResolver {
    fn default() -> Self {
        Self::OpeningSnapshots(Arc::new(StdMutex::new(HashMap::new())))
    }
}

impl CurrentAgentConfigResolver {
    fn observe_open(&self, agent_id: &str, config: &AgentConfig) {
        let Self::OpeningSnapshots(configs) = self else {
            return;
        };
        match configs.lock() {
            Ok(mut configs) => {
                configs.insert(agent_id.to_string(), config.clone());
            }
            Err(poisoned) => {
                poisoned
                    .into_inner()
                    .insert(agent_id.to_string(), config.clone());
            }
        }
    }

    async fn resolve(&self, agent_id: &str) -> Result<AgentConfig, FileResourceErrorV1> {
        match self {
            Self::OpeningSnapshots(configs) => {
                let configs = configs.lock().map_err(|_| {
                    error(
                        "runtime_unavailable",
                        "standalone agent configuration lock is unavailable",
                    )
                })?;
                configs.get(agent_id).cloned().ok_or_else(|| {
                    error(
                        "unauthorized_path",
                        "agent authorization is no longer active",
                    )
                })
            }
            Self::AppState(app_handle) => {
                let state = app_handle
                    .try_state::<crate::state::AppState>()
                    .ok_or_else(|| {
                        error(
                            "runtime_unavailable",
                            "application state is unavailable for file authorization",
                        )
                    })?;
                let config = {
                    let agents = state.agents.lock().await;
                    agents
                        .get(agent_id)
                        .map(|agent| agent.config.clone())
                        .ok_or_else(|| {
                            error(
                                "unauthorized_path",
                                "agent authorization is no longer active",
                            )
                        })?
                };
                let config = config.lock().map_err(|_| {
                    error(
                        "runtime_unavailable",
                        "agent configuration lock is unavailable",
                    )
                })?;
                Ok(config.clone())
            }
        }
    }

    #[cfg(test)]
    fn revoke_opening_snapshot(&self, agent_id: &str) {
        let Self::OpeningSnapshots(configs) = self else {
            panic!("test agent revocation requires the standalone resolver");
        };
        match configs.lock() {
            Ok(mut configs) => {
                configs.remove(agent_id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(agent_id);
            }
        }
    }
}

#[derive(Clone)]
struct UserFileGrant {
    canonical_path: String,
    authorized: AuthorizedPath,
    last_used_at: Instant,
    in_flight_uses: usize,
    active_subscriptions: usize,
}

struct UserFileGrantReservation {
    grants: OwnedMutexGuard<HashMap<String, UserFileGrant>>,
    capability_id: String,
    evict_capability_id: Option<String>,
    canonical_path: String,
}

impl UserFileGrantReservation {
    fn publish(mut self, authorized: AuthorizedPath) -> String {
        let now = Instant::now();
        if let Some(existing) = self.grants.get_mut(&self.capability_id) {
            existing.authorized = authorized;
            existing.last_used_at = now;
            return self.capability_id.clone();
        }
        if let Some(evict_capability_id) = &self.evict_capability_id {
            self.grants.remove(evict_capability_id);
        }
        self.grants.insert(
            self.capability_id.clone(),
            UserFileGrant {
                canonical_path: self.canonical_path,
                authorized,
                last_used_at: now,
                in_flight_uses: 0,
                active_subscriptions: 0,
            },
        );
        self.capability_id.clone()
    }
}

struct SaveTargetGrant {
    selected_path: PathBuf,
    requested_parent: PathBuf,
    canonical_parent: PathBuf,
    basename: OsString,
    parent: File,
    parent_identity: FilesystemIdentity,
    binding: SaveTargetBinding,
    expires_at: Instant,
}

enum SaveTargetBinding {
    Missing,
    Existing {
        authorized: AuthorizedPath,
        snapshot: Box<VerifiedFileSnapshot>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FilesystemIdentity {
    volume: u64,
    file: u64,
}

#[derive(Clone)]
struct FileReadTicket {
    issuance_id: Uuid,
    webview_label: Option<String>,
    renderer_lease: RendererLeaseKey,
    subscription_id: String,
    resource_id: String,
    snapshot: Arc<ImmutableTicketSnapshot>,
    size_bytes: u64,
    mime_type: String,
    expires_at: Instant,
}

struct ImmutableTicketSnapshot {
    file: StdMutex<File>,
    size_bytes: u64,
    reserved_bytes: u64,
    usage: Arc<AtomicU64>,
}

impl ImmutableTicketSnapshot {
    fn read_range(&self, start: u64, end: u64) -> Result<Vec<u8>, FileResourceErrorV1> {
        if start > end || end >= self.size_bytes {
            return Err(error(
                "range_not_satisfiable",
                "byte range is outside the immutable ticket snapshot",
            ));
        }
        let length = end - start + 1;
        let length: usize = length.try_into().map_err(|_| {
            error(
                "file_too_large",
                "selected byte range cannot fit in process memory",
            )
        })?;
        let mut bytes = vec![0_u8; length];
        let mut file = self
            .file
            .lock()
            .map_err(|_| error("runtime_unavailable", "ticket snapshot is unavailable"))?;
        file.seek(SeekFrom::Start(start)).map_err(|cause| {
            error(
                "runtime_unavailable",
                format!("cannot seek immutable ticket snapshot: {cause}"),
            )
        })?;
        file.read_exact(&mut bytes).map_err(|cause| {
            error(
                "runtime_unavailable",
                format!("cannot read immutable ticket snapshot: {cause}"),
            )
        })?;
        Ok(bytes)
    }
}

impl Drop for ImmutableTicketSnapshot {
    fn drop(&mut self) {
        self.usage.fetch_sub(self.reserved_bytes, Ordering::AcqRel);
    }
}

struct TicketSnapshotReservation {
    usage: Arc<AtomicU64>,
    size_bytes: u64,
    reserved_bytes: u64,
    committed: bool,
}

impl TicketSnapshotReservation {
    fn commit(mut self, file: File) -> Arc<ImmutableTicketSnapshot> {
        self.committed = true;
        Arc::new(ImmutableTicketSnapshot {
            file: StdMutex::new(file),
            size_bytes: self.size_bytes,
            reserved_bytes: self.reserved_bytes,
            usage: self.usage.clone(),
        })
    }
}

impl Drop for TicketSnapshotReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.usage.fetch_sub(self.reserved_bytes, Ordering::AcqRel);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RendererLeaseKey {
    webview_label: Option<String>,
    renderer_lease_id: String,
}

#[derive(Clone)]
struct RendererLease {
    issuance_id: Uuid,
    subscription_id: String,
    expires_at: Instant,
}

impl FileResourceRuntime {
    #[must_use]
    pub fn with_timing(stability_delay: Duration, ticket_ttl: Duration) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(FileResourceRuntimeInner {
                entries: Mutex::new(HashMap::new()),
                subscription_resources: Mutex::new(HashMap::new()),
                user_file_grants: Arc::new(Mutex::new(HashMap::new())),
                save_target_grants: Mutex::new(HashMap::new()),
                read_tickets: Mutex::new(HashMap::new()),
                renderer_leases: Mutex::new(HashMap::new()),
                ticket_publication: Mutex::new(()),
                limits: FileResourceLimits::default(),
                stability_delay,
                ticket_ttl,
                max_user_file_grants: DEFAULT_MAX_USER_FILE_GRANTS,
                max_save_target_grants: DEFAULT_MAX_SAVE_TARGET_GRANTS,
                save_target_ttl: DEFAULT_SAVE_TARGET_TTL,
                ticket_snapshot_usage: Arc::new(AtomicU64::new(0)),
                max_ticket_snapshot_bytes: MAX_TICKET_SNAPSHOT_BYTES,
                events,
                app_handle: RwLock::new(None),
                agent_config_resolver: RwLock::new(CurrentAgentConfigResolver::default()),
                recovery_root: RwLock::new(default_recovery_root()),
                recovery_io: Mutex::new(()),
                recovery_store_limits: RwLock::new(FileRecoveryStoreLimits::default()),
                #[cfg(test)]
                issue_ticket_after_validation_hook: Mutex::new(None),
                #[cfg(test)]
                ticket_publication_hook: Mutex::new(None),
                #[cfg(test)]
                forced_refresh_error: Mutex::new(None),
                #[cfg(test)]
                open_after_entry_miss_hook: Mutex::new(None),
                #[cfg(test)]
                grant_eviction_before_lock_hook: Mutex::new(None),
                #[cfg(test)]
                save_after_validation_hook: Mutex::new(None),
                #[cfg(test)]
                fail_recovery_before_manifest: AtomicBool::new(false),
                #[cfg(test)]
                refresh_scan_count: AtomicU64::new(0),
            }),
        }
    }

    #[cfg(test)]
    fn with_test_limits(
        stability_delay: Duration,
        ticket_ttl: Duration,
        max_user_file_grants: usize,
        max_ticket_snapshot_bytes: u64,
    ) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(FileResourceRuntimeInner {
                entries: Mutex::new(HashMap::new()),
                subscription_resources: Mutex::new(HashMap::new()),
                user_file_grants: Arc::new(Mutex::new(HashMap::new())),
                save_target_grants: Mutex::new(HashMap::new()),
                read_tickets: Mutex::new(HashMap::new()),
                renderer_leases: Mutex::new(HashMap::new()),
                ticket_publication: Mutex::new(()),
                limits: FileResourceLimits::default(),
                stability_delay,
                ticket_ttl,
                max_user_file_grants,
                max_save_target_grants: DEFAULT_MAX_SAVE_TARGET_GRANTS,
                save_target_ttl: DEFAULT_SAVE_TARGET_TTL,
                ticket_snapshot_usage: Arc::new(AtomicU64::new(0)),
                max_ticket_snapshot_bytes,
                events,
                app_handle: RwLock::new(None),
                agent_config_resolver: RwLock::new(CurrentAgentConfigResolver::default()),
                recovery_root: RwLock::new(default_recovery_root()),
                recovery_io: Mutex::new(()),
                recovery_store_limits: RwLock::new(FileRecoveryStoreLimits::default()),
                issue_ticket_after_validation_hook: Mutex::new(None),
                ticket_publication_hook: Mutex::new(None),
                forced_refresh_error: Mutex::new(None),
                open_after_entry_miss_hook: Mutex::new(None),
                grant_eviction_before_lock_hook: Mutex::new(None),
                save_after_validation_hook: Mutex::new(None),
                fail_recovery_before_manifest: AtomicBool::new(false),
                refresh_scan_count: AtomicU64::new(0),
            }),
        }
    }

    #[cfg(test)]
    fn with_recovery_root(
        stability_delay: Duration,
        ticket_ttl: Duration,
        recovery_root: PathBuf,
    ) -> Self {
        let runtime = Self::with_timing(stability_delay, ticket_ttl);
        match runtime.inner.recovery_root.write() {
            Ok(mut current) => *current = Some(recovery_root),
            Err(poisoned) => *poisoned.into_inner() = Some(recovery_root),
        }
        runtime
    }

    pub fn attach_app_handle(&self, app_handle: tauri::AppHandle) {
        match self.inner.agent_config_resolver.write() {
            Ok(mut current) => {
                *current = CurrentAgentConfigResolver::AppState(app_handle.clone());
            }
            Err(poisoned) => {
                *poisoned.into_inner() = CurrentAgentConfigResolver::AppState(app_handle.clone());
            }
        }
        match self.inner.app_handle.write() {
            Ok(mut current) => *current = Some(app_handle),
            Err(poisoned) => *poisoned.into_inner() = Some(app_handle),
        }
    }

    fn current_agent_config_resolver(&self) -> CurrentAgentConfigResolver {
        self.inner
            .agent_config_resolver
            .read()
            .map(|resolver| resolver.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    #[cfg(test)]
    fn revoke_test_agent_config(&self, agent_id: &str) {
        self.current_agent_config_resolver()
            .revoke_opening_snapshot(agent_id);
    }

    #[cfg(test)]
    fn fail_next_recovery_before_manifest(&self) {
        self.inner
            .fail_recovery_before_manifest
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn configure_recovery_store_for_test(
        &self,
        max_records: usize,
        max_body_bytes: u64,
        orphan_grace_period: Duration,
    ) {
        let limits = FileRecoveryStoreLimits {
            max_records,
            max_body_bytes,
            orphan_grace_period,
        };
        match self.inner.recovery_store_limits.write() {
            Ok(mut current) => *current = limits,
            Err(poisoned) => *poisoned.into_inner() = limits,
        }
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<FileResourceEventV1> {
        self.inner.events.subscribe()
    }

    pub async fn open_agent_file(
        &self,
        agent_id: &str,
        config: &AgentConfig,
        path: &Path,
        app_handle: Option<tauri::AppHandle>,
    ) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
        if config.session_id != agent_id {
            return Err(error(
                "unauthorized_path",
                "agent configuration does not match the requested agent",
            ));
        }
        if let Some(app_handle) = app_handle {
            self.attach_app_handle(app_handle);
        }
        self.current_agent_config_resolver()
            .observe_open(agent_id, config);
        let roots = AuthorizedRootService::from_agent_config(config)?;
        let authorized = roots.authorize_existing_file(path)?;
        self.open_authorized(
            authorized,
            FileAccessClaim::Agent {
                agent_id: agent_id.to_string(),
            },
        )
        .await
    }

    pub async fn record_user_file(
        &self,
        selected_path: &Path,
    ) -> Result<UserFileGrantV1, FileResourceErrorV1> {
        let authorized = authorize_user_file_path(selected_path)?;
        let snapshot = verified_snapshot(authorized.clone(), self.inner.limits.clone()).await?;
        let canonical_path = snapshot.descriptor().canonical_path.clone();
        let capability_id = self
            .upsert_user_file_grant(canonical_path.clone(), authorized)
            .await?;
        Ok(UserFileGrantV1 {
            schema: 1,
            capability_id,
            canonical_path,
        })
    }

    /// Mints a short-lived, one-shot capability for exactly one native-dialog
    /// save target.
    ///
    /// The backend retains the verified parent directory identity and exact
    /// basename. Existing targets additionally retain their ordinary-file
    /// identity and private revision token; absent targets must remain absent
    /// until the atomic create commits.
    pub async fn record_save_target(
        &self,
        selected_path: &Path,
    ) -> Result<SaveTargetGrantV1, FileResourceErrorV1> {
        let selected_path = absolute_path(selected_path)?;
        let requested_parent = selected_path.parent().ok_or_else(|| {
            error(
                "unauthorized_save_target",
                "selected save target has no parent directory",
            )
        })?;
        let basename = selected_path.file_name().ok_or_else(|| {
            error(
                "unauthorized_save_target",
                "selected save target has no exact basename",
            )
        })?;
        if basename.is_empty() || basename == "." || basename == ".." {
            return Err(error(
                "unauthorized_save_target",
                "selected save target basename is invalid",
            ));
        }
        let canonical_parent = std::fs::canonicalize(requested_parent).map_err(|cause| {
            error(
                "unavailable_path",
                format!("cannot resolve selected save directory: {cause}"),
            )
        })?;
        let parent = open_directory(&canonical_parent).map_err(|cause| {
            error(
                "unavailable_path",
                format!("cannot retain selected save directory: {cause}"),
            )
        })?;
        let parent_identity = FilesystemIdentity::from_file(&parent).map_err(|cause| {
            error(
                "unavailable_path",
                format!("cannot identify selected save directory: {cause}"),
            )
        })?;
        let selected_path = canonical_parent.join(basename);
        let binding = match std::fs::symlink_metadata(&selected_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(error(
                    "unauthorized_save_target",
                    "selected save target must be an ordinary file",
                ));
            }
            Ok(_) => {
                let authorized = authorize_user_file_path(&selected_path)?;
                let snapshot =
                    verified_snapshot(authorized.clone(), self.inner.limits.clone()).await?;
                SaveTargetBinding::Existing {
                    authorized,
                    snapshot: Box::new(snapshot),
                }
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                SaveTargetBinding::Missing
            }
            Err(cause) => {
                return Err(error(
                    "unavailable_path",
                    format!("cannot inspect selected save target: {cause}"),
                ));
            }
        };
        let selected_path_text = selected_path.to_str().ok_or_else(|| {
            error(
                "unavailable_path",
                "selected save target cannot be represented losslessly as UTF-8",
            )
        })?;
        let save_target_grant_id = Uuid::new_v4().to_string();
        let now = Instant::now();
        let mut grants = self.inner.save_target_grants.lock().await;
        grants.retain(|_, grant| grant.expires_at > now);
        if grants.len() >= self.inner.max_save_target_grants {
            return Err(error(
                "grant_limit_reached",
                "too many native save target grants are awaiting use",
            ));
        }
        grants.insert(
            save_target_grant_id.clone(),
            SaveTargetGrant {
                selected_path: selected_path.clone(),
                requested_parent: requested_parent.to_path_buf(),
                canonical_parent,
                basename: basename.to_os_string(),
                parent,
                parent_identity,
                binding,
                expires_at: now + self.inner.save_target_ttl,
            },
        );
        Ok(SaveTargetGrantV1 {
            schema: 1,
            save_target_grant_id,
            selected_path: selected_path_text.to_string(),
        })
    }

    /// Atomically writes UTF-8 text through a one-shot exact-target grant and
    /// returns a new ordinary-file capability without touching any open source
    /// resource or artifact identity.
    pub async fn save_file_resource_as_text(
        &self,
        save_target_grant_id: &str,
        text: &str,
    ) -> Result<FileResourceSaveAsResultV1, FileResourceErrorV1> {
        let reserved_canonical_path = {
            let grants = self.inner.save_target_grants.lock().await;
            let grant = grants.get(save_target_grant_id).ok_or_else(|| {
                error(
                    "unauthorized_save_target",
                    "save target grant is unavailable or already consumed",
                )
            })?;
            if grant.expires_at <= Instant::now() {
                return Err(error(
                    "unauthorized_save_target",
                    "save target grant has expired",
                ));
            }
            prospective_save_target_canonical_path(grant)?
        };
        let user_grant_reservation = self
            .reserve_user_file_grant(reserved_canonical_path)
            .await?;
        let mut grant = self
            .inner
            .save_target_grants
            .lock()
            .await
            .remove(save_target_grant_id)
            .ok_or_else(|| {
                error(
                    "unauthorized_save_target",
                    "save target grant is unavailable or already consumed",
                )
            })?;
        if grant.expires_at <= Instant::now() {
            return Err(error(
                "unauthorized_save_target",
                "save target grant has expired",
            ));
        }
        validate_submitted_text(text, &self.inner.limits)?;
        verify_save_target_parent(&grant)?;

        let binding = std::mem::replace(&mut grant.binding, SaveTargetBinding::Missing);
        let (authorized, snapshot) = match binding {
            SaveTargetBinding::Existing {
                authorized,
                snapshot,
            } => {
                let expected_hash = snapshot.descriptor().content_hash.clone();
                let revision_token = snapshot.revision_token().clone();
                let limits = self.inner.limits.clone();
                let submitted = text.to_string();
                let write = tauri::async_runtime::spawn_blocking(move || {
                    authorized.guarded_atomic_replace_text(
                        &revision_token,
                        &expected_hash,
                        &submitted,
                        &limits,
                    )
                })
                .await
                .map_err(join_error)??;
                let (_, authorized, snapshot) = write.into_parts();
                (authorized, snapshot)
            }
            SaveTargetBinding::Missing => {
                let selected_path = grant.selected_path.clone();
                let submitted = text.to_string();
                let limits = self.inner.limits.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    atomic_create_text_exact(&grant, &submitted)?;
                    let authorized = authorize_user_file_path(&selected_path)?;
                    let snapshot =
                        VerifiedFileSnapshot::from_authorized_path(&authorized, &limits)?;
                    Ok::<_, FileResourceErrorV1>((authorized, snapshot))
                })
                .await
                .map_err(join_error)??
            }
        };
        let canonical_path = snapshot.descriptor().canonical_path.clone();
        let content_hash = snapshot.descriptor().content_hash.clone();
        let capability_id = user_grant_reservation.publish(authorized);
        Ok(FileResourceSaveAsResultV1 {
            schema: 1,
            capability_id,
            resource_id: file_resource_id(&canonical_path),
            canonical_path,
            content_hash,
        })
    }

    pub async fn open_user_file(
        &self,
        capability_id: &str,
        path: &Path,
        app_handle: Option<tauri::AppHandle>,
    ) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
        if let Some(app_handle) = app_handle {
            self.attach_app_handle(app_handle);
        }
        let requested = std::fs::canonicalize(path).map_err(|cause| {
            error(
                "unavailable_path",
                format!("cannot resolve selected file: {cause}"),
            )
        })?;
        let requested = requested.to_str().ok_or_else(|| {
            error(
                "unavailable_path",
                "selected file cannot be represented losslessly as UTF-8",
            )
        })?;
        {
            let mut grants = self.inner.user_file_grants.lock().await;
            let grant = grants
                .get_mut(capability_id)
                .ok_or_else(|| error("unauthorized_path", "user file capability is unavailable"))?;
            if requested != grant.canonical_path {
                return Err(error(
                    "unauthorized_path",
                    "user file capability grants only the selected canonical file",
                ));
            }
            grant.in_flight_uses = grant.in_flight_uses.saturating_add(1);
        }
        let authorized = match authorize_user_file_path(path) {
            Ok(authorized) => authorized,
            Err(error) => {
                self.finish_user_grant_open(capability_id, None).await;
                return Err(error);
            }
        };
        if authorized.canonical_path != Path::new(requested) {
            self.finish_user_grant_open(capability_id, None).await;
            return Err(error(
                "unauthorized_path",
                "selected path changed while its exact capability was being opened",
            ));
        }
        let current_grant = {
            let mut grants = self.inner.user_file_grants.lock().await;
            match grants.get_mut(capability_id) {
                Some(current) if current.canonical_path == requested => {
                    if current.authorized.requested_path() == authorized.requested_path() {
                        current.authorized = authorized.clone();
                    }
                    current.last_used_at = Instant::now();
                    Ok(())
                }
                Some(_) => Err(error(
                    "unauthorized_path",
                    "user file capability changed while it was being opened",
                )),
                None => Err(error(
                    "unauthorized_path",
                    "user file capability was revoked",
                )),
            }
        };
        if let Err(error) = current_grant {
            self.finish_user_grant_open(capability_id, None).await;
            return Err(error);
        }
        let result = self
            .open_authorized(
                authorized,
                FileAccessClaim::User {
                    capability_id: capability_id.to_string(),
                },
            )
            .await;
        let opened_authorized = match &result {
            Ok(snapshot) => self
                .inner
                .entries
                .lock()
                .await
                .get(&snapshot.resource_id)
                .and_then(|entry| entry.subscribers.get(&snapshot.subscription_id))
                .map(|access| access.authorized.clone()),
            Err(_) => None,
        };
        self.finish_user_grant_open(capability_id, opened_authorized)
            .await;
        result
    }

    async fn finish_user_grant_open(
        &self,
        capability_id: &str,
        authorized: Option<AuthorizedPath>,
    ) {
        let mut grants = self.inner.user_file_grants.lock().await;
        if let Some(grant) = grants.get_mut(capability_id) {
            grant.in_flight_uses = grant.in_flight_uses.saturating_sub(1);
            if let Some(authorized) = authorized {
                grant.active_subscriptions = grant.active_subscriptions.saturating_add(1);
                if grant.authorized.requested_path() == authorized.requested_path() {
                    grant.authorized = authorized;
                }
                grant.last_used_at = Instant::now();
            }
        }
    }

    async fn upsert_user_file_grant(
        &self,
        canonical_path: String,
        authorized: AuthorizedPath,
    ) -> Result<String, FileResourceErrorV1> {
        #[cfg(test)]
        let grant_eviction_before_lock_hook = {
            self.inner
                .grant_eviction_before_lock_hook
                .lock()
                .await
                .clone()
        };
        #[cfg(test)]
        if let Some(hook) = grant_eviction_before_lock_hook {
            hook.reached.wait().await;
            hook.resume.wait().await;
        }
        let reservation = self.reserve_user_file_grant(canonical_path).await?;
        Ok(reservation.publish(authorized))
    }

    async fn reserve_user_file_grant(
        &self,
        canonical_path: String,
    ) -> Result<UserFileGrantReservation, FileResourceErrorV1> {
        let grants = self.inner.user_file_grants.clone().lock_owned().await;
        let existing_capability_id = grants
            .iter()
            .find(|(_, grant)| grant.canonical_path == canonical_path)
            .map(|(capability_id, _)| capability_id.clone());
        if let Some(capability_id) = existing_capability_id {
            return Ok(UserFileGrantReservation {
                grants,
                capability_id,
                evict_capability_id: None,
                canonical_path,
            });
        }

        let evict_capability_id = if grants.len() >= self.inner.max_user_file_grants {
            Some(
                grants
                    .iter()
                    .filter(|(_, grant)| {
                        grant.in_flight_uses == 0 && grant.active_subscriptions == 0
                    })
                    .min_by_key(|(_, grant)| grant.last_used_at)
                    .map(|(capability_id, _)| capability_id.clone())
                    .ok_or_else(|| {
                        error(
                            "grant_limit_reached",
                            "all exact-file grants are active; close a file before selecting another",
                        )
                    })?,
            )
        } else {
            None
        };
        Ok(UserFileGrantReservation {
            grants,
            capability_id: Uuid::new_v4().to_string(),
            evict_capability_id,
            canonical_path,
        })
    }

    /// Reopens an exact file selected through the native picker without
    /// exposing or persisting its capability identifier in Workbench state.
    ///
    /// The match is backend-owned and exact: sibling files never inherit a
    /// picker grant. Capability identifiers are sorted so duplicate live grants
    /// resolve deterministically.
    pub async fn open_matching_user_file(
        &self,
        path: &Path,
        app_handle: Option<tauri::AppHandle>,
    ) -> Result<Option<FileResourceSnapshotV1>, FileResourceErrorV1> {
        let has_grants = !self.inner.user_file_grants.lock().await.is_empty();
        if !has_grants {
            return Ok(None);
        }
        let requested = std::fs::canonicalize(path).map_err(|cause| {
            error(
                "unavailable_path",
                format!("cannot resolve selected file: {cause}"),
            )
        })?;
        let requested = requested.to_str().ok_or_else(|| {
            error(
                "unavailable_path",
                "selected file cannot be represented losslessly as UTF-8",
            )
        })?;
        let capability_id = {
            let grants = self.inner.user_file_grants.lock().await;
            let mut matching = grants
                .iter()
                .filter_map(|(capability_id, grant)| {
                    (grant.canonical_path == requested).then_some(capability_id.clone())
                })
                .collect::<Vec<_>>();
            matching.sort();
            matching.into_iter().next()
        };
        let Some(capability_id) = capability_id else {
            return Ok(None);
        };
        self.open_user_file(&capability_id, path, app_handle)
            .await
            .map(Some)
    }

    async fn open_authorized(
        &self,
        authorized: AuthorizedPath,
        claim: FileAccessClaim,
    ) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
        let canonical_path = authorized.canonical_path.to_str().ok_or_else(|| {
            error(
                "unavailable_path",
                "canonical path cannot be represented losslessly as UTF-8",
            )
        })?;
        let resource_id = file_resource_id(canonical_path);
        let subscription_id = Uuid::new_v4().to_string();

        if let Some(result) = self
            .attach_existing_subscription(&resource_id, &subscription_id, &authorized, &claim)
            .await?
        {
            return Ok(result);
        }

        #[cfg(test)]
        let open_after_entry_miss_hook =
            { self.inner.open_after_entry_miss_hook.lock().await.clone() };
        #[cfg(test)]
        if let Some(hook) = open_after_entry_miss_hook {
            hook.wait().await;
        }

        let incarnation_id = Uuid::new_v4();
        let pending_event = Arc::new(AtomicBool::new(false));
        let watcher = self.create_watcher(
            &resource_id,
            &authorized.canonical_path,
            incarnation_id,
            pending_event.clone(),
        )?;
        let snapshot = verified_snapshot(authorized.clone(), self.inner.limits.clone()).await?;
        let (descriptor, revision_token) = snapshot.into_parts();
        let mut watcher = Some(watcher);
        let result = loop {
            if let Some(result) = self
                .attach_existing_subscription(&resource_id, &subscription_id, &authorized, &claim)
                .await?
            {
                return Ok(result);
            }
            let mut entries = self.inner.entries.lock().await;
            if entries.contains_key(&resource_id) {
                drop(entries);
                continue;
            }
            let mut subscribers = HashMap::new();
            subscribers.insert(
                subscription_id.clone(),
                FileSubscriptionAccess {
                    claim: claim.clone(),
                    authorized: authorized.clone(),
                },
            );
            entries.insert(
                resource_id.clone(),
                FileResourceEntry {
                    _watcher: watcher.take().ok_or_else(|| {
                        error("runtime_unavailable", "file watcher was already installed")
                    })?,
                    revision_token: revision_token.clone(),
                    descriptor: descriptor.clone(),
                    revision: 1,
                    incarnation_id,
                    subscribers,
                    debounce_generation: 0,
                    operation: Arc::new(Mutex::new(())),
                },
            );
            break FileResourceSnapshotV1 {
                resource_id: resource_id.clone(),
                subscription_id: subscription_id.clone(),
                revision: 1,
                descriptor: descriptor.clone(),
            };
        };
        self.inner
            .subscription_resources
            .lock()
            .await
            .insert(subscription_id.clone(), resource_id.clone());
        if pending_event.swap(false, Ordering::AcqRel) {
            self.schedule_refresh_for_incarnation(resource_id, incarnation_id);
        }
        Ok(result)
    }

    async fn attach_existing_subscription(
        &self,
        resource_id: &str,
        subscription_id: &str,
        authorized: &AuthorizedPath,
        claim: &FileAccessClaim,
    ) -> Result<Option<FileResourceSnapshotV1>, FileResourceErrorV1> {
        loop {
            let operation = {
                let entries = self.inner.entries.lock().await;
                entries
                    .get(resource_id)
                    .map(|entry| entry.operation.clone())
            };
            let Some(operation) = operation else {
                return Ok(None);
            };
            let _operation = operation.lock().await;
            let current_authorized = authorized.reauthorize_same_target()?;
            let (result, incarnation_id) = {
                let mut entries = self.inner.entries.lock().await;
                let Some(entry) = entries.get_mut(resource_id) else {
                    continue;
                };
                if !Arc::ptr_eq(&entry.operation, &operation) {
                    continue;
                }
                if current_authorized.canonical_path != Path::new(&entry.descriptor.canonical_path)
                {
                    return Err(error(
                        "unauthorized_path",
                        "file subscription admission resolved to another resource",
                    ));
                }
                entry.subscribers.insert(
                    subscription_id.to_string(),
                    FileSubscriptionAccess {
                        claim: claim.clone(),
                        authorized: current_authorized,
                    },
                );
                (
                    FileResourceSnapshotV1 {
                        resource_id: resource_id.to_string(),
                        subscription_id: subscription_id.to_string(),
                        revision: entry.revision,
                        descriptor: entry.descriptor.clone(),
                    },
                    entry.incarnation_id,
                )
            };
            self.inner
                .subscription_resources
                .lock()
                .await
                .insert(subscription_id.to_string(), resource_id.to_string());
            self.schedule_refresh_for_incarnation(resource_id.to_string(), incarnation_id);
            return Ok(Some(result));
        }
    }

    fn create_watcher(
        &self,
        resource_id: &str,
        path: &Path,
        incarnation_id: Uuid,
        pending_event: Arc<AtomicBool>,
    ) -> Result<notify::RecommendedWatcher, FileResourceErrorV1> {
        let weak: Weak<FileResourceRuntimeInner> = Arc::downgrade(&self.inner);
        let resource_id = resource_id.to_string();
        let watched_path = path.to_path_buf();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    if !event.paths.is_empty()
                        && !event.paths.iter().any(|path| path == &watched_path)
                    {
                        return;
                    }
                    pending_event.store(true, Ordering::Release);
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    FileResourceRuntime { inner }
                        .schedule_refresh_for_incarnation(resource_id.clone(), incarnation_id);
                }
            })
            .map_err(|cause| {
                error(
                    "watch_unavailable",
                    format!("cannot create watcher: {cause}"),
                )
            })?;
        let watch_root = path.parent().unwrap_or(path);
        watcher
            .watch(watch_root, notify::RecursiveMode::NonRecursive)
            .map_err(|cause| error("watch_unavailable", format!("cannot watch file: {cause}")))?;
        Ok(watcher)
    }

    #[cfg(test)]
    fn schedule_refresh(&self, resource_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            let incarnation_id = {
                let entries = runtime.inner.entries.lock().await;
                let Some(entry) = entries.get(&resource_id) else {
                    return;
                };
                entry.incarnation_id
            };
            runtime.schedule_refresh_for_incarnation(resource_id, incarnation_id);
        });
    }

    fn schedule_refresh_for_incarnation(&self, resource_id: String, incarnation_id: Uuid) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            let generation = {
                let mut entries = runtime.inner.entries.lock().await;
                let Some(entry) = entries.get_mut(&resource_id) else {
                    return;
                };
                if entry.incarnation_id != incarnation_id {
                    return;
                }
                entry.debounce_generation = entry.debounce_generation.saturating_add(1);
                entry.debounce_generation
            };
            tokio::time::sleep(runtime.inner.stability_delay).await;
            runtime
                .refresh_if_stable(&resource_id, incarnation_id, generation)
                .await;
        });
    }

    async fn refresh_if_stable(&self, resource_id: &str, incarnation_id: Uuid, generation: u64) {
        let operation = {
            let entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get(resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id || entry.debounce_generation != generation {
                return;
            }
            entry.operation.clone()
        };
        let _operation = operation.lock().await;
        let candidates = {
            let entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get(resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id || entry.debounce_generation != generation {
                return;
            }
            let mut candidates = entry
                .subscribers
                .iter()
                .map(|(subscription_id, access)| FileRefreshCandidate {
                    subscription_id: subscription_id.clone(),
                    access: access.clone(),
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| {
                left.access
                    .authorized
                    .requested_path()
                    .cmp(right.access.authorized.requested_path())
                    .then_with(|| left.subscription_id.cmp(&right.subscription_id))
            });
            candidates
        };
        if candidates.is_empty() {
            return;
        }
        let mut expected_subscriptions = candidates
            .iter()
            .map(|candidate| candidate.subscription_id.clone())
            .collect::<Vec<_>>();
        expected_subscriptions.sort();

        let mut first_failure = None;
        let mut refreshed = None;
        let mut refreshed_authorizations = HashMap::new();
        for candidate in &candidates {
            let authorized = match self.validate_refresh_candidate(&candidate.access).await {
                Ok(authorized) => authorized,
                Err(failure) => {
                    if first_failure.is_none() {
                        first_failure = Some(failure);
                    }
                    continue;
                }
            };
            refreshed_authorizations.insert(candidate.subscription_id.clone(), authorized.clone());
            match self.refresh_from_authorization(authorized).await {
                Ok((authorized, snapshot)) => {
                    refreshed_authorizations.insert(candidate.subscription_id.clone(), authorized);
                    refreshed = Some(snapshot);
                    break;
                }
                Err(failure) => {
                    if first_failure.is_none() {
                        first_failure = Some(failure);
                    }
                }
            }
        }
        let Some(snapshot) = refreshed else {
            let failure = first_failure.unwrap_or_else(|| {
                error(
                    "unavailable_path",
                    "no active subscription authorization can refresh the file resource",
                )
            });
            self.publish_refresh_failure(
                resource_id,
                incarnation_id,
                generation,
                &expected_subscriptions,
                &failure,
            )
            .await;
            return;
        };
        let (descriptor, revision_token) = snapshot.into_parts();
        let application = {
            let mut entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get_mut(resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id || entry.debounce_generation != generation {
                return;
            }
            if !same_subscriptions(entry, &expected_subscriptions) {
                None
            } else {
                for (subscription_id, authorized) in &refreshed_authorizations {
                    if let Some(access) = entry.subscribers.get_mut(subscription_id) {
                        access.authorized = authorized.clone();
                    }
                }
                let user_grant_updates = refreshed_authorizations
                    .iter()
                    .filter_map(|(subscription_id, authorized)| {
                        let access = entry.subscribers.get(subscription_id)?;
                        match &access.claim {
                            FileAccessClaim::User { capability_id } => {
                                Some((capability_id.clone(), authorized.clone()))
                            }
                            FileAccessClaim::Agent { .. } => None,
                        }
                    })
                    .collect::<Vec<_>>();
                entry.revision_token = revision_token;
                let availability_changed =
                    entry.descriptor.unavailable_reason != descriptor.unavailable_reason;
                let content_changed = entry.descriptor.content_hash != descriptor.content_hash;
                if !content_changed && !availability_changed {
                    entry.descriptor = descriptor;
                    Some((None, user_grant_updates))
                } else {
                    entry.revision = entry.revision.saturating_add(1);
                    entry.descriptor = descriptor.clone();
                    Some((
                        Some(FileResourceEventV1 {
                            schema: 1,
                            resource_id: resource_id.to_string(),
                            revision: entry.revision,
                            descriptor,
                        }),
                        user_grant_updates,
                    ))
                }
            }
        };
        let Some((event, user_grant_updates)) = application else {
            self.schedule_refresh_for_incarnation(resource_id.to_string(), incarnation_id);
            return;
        };
        if !user_grant_updates.is_empty() {
            let now = Instant::now();
            let mut grants = self.inner.user_file_grants.lock().await;
            for (capability_id, authorized) in user_grant_updates {
                if let Some(grant) = grants.get_mut(&capability_id) {
                    if grant.authorized.requested_path() == authorized.requested_path() {
                        grant.authorized = authorized;
                        grant.last_used_at = now;
                    }
                }
            }
        }
        if let Some(event) = event {
            self.emit(event);
        }
    }

    async fn validate_refresh_candidate(
        &self,
        access: &FileSubscriptionAccess,
    ) -> Result<AuthorizedPath, FileResourceErrorV1> {
        let rebound = access.authorized.reauthorize_same_target()?;
        match &access.claim {
            FileAccessClaim::Agent { agent_id } => {
                let config = self
                    .current_agent_config_resolver()
                    .resolve(agent_id)
                    .await?;
                if &config.session_id != agent_id {
                    return Err(error(
                        "unauthorized_path",
                        "current agent authorization does not match the subscription",
                    ));
                }
                let current = AuthorizedRootService::from_agent_config(&config)?
                    .authorize_existing_file(access.authorized.requested_path())?;
                if current.canonical_path != access.authorized.canonical_path {
                    return Err(error(
                        "unauthorized_path",
                        "current agent authorization resolves to another file",
                    ));
                }
                Ok(current)
            }
            FileAccessClaim::User { capability_id } => {
                let expected = access.authorized.canonical_path.to_str().ok_or_else(|| {
                    error(
                        "unavailable_path",
                        "canonical path cannot be represented losslessly as UTF-8",
                    )
                })?;
                let grants = self.inner.user_file_grants.lock().await;
                let grant = grants.get(capability_id).ok_or_else(|| {
                    error("unauthorized_path", "user file capability was revoked")
                })?;
                if grant.canonical_path != expected {
                    return Err(error(
                        "unauthorized_path",
                        "user file capability does not match the resource",
                    ));
                }
                Ok(rebound)
            }
        }
    }

    async fn refresh_from_authorization(
        &self,
        authorized: AuthorizedPath,
    ) -> Result<(AuthorizedPath, VerifiedFileSnapshot), FileResourceErrorV1> {
        match self.refresh_verified_snapshot(authorized.clone()).await {
            Ok(snapshot) => Ok((authorized, snapshot)),
            Err(initial_error)
                if matches!(
                    initial_error.code(),
                    "unauthorized_path" | "unavailable_path"
                ) =>
            {
                let replacement = authorized.reauthorize_same_target()?;
                let snapshot = self.refresh_verified_snapshot(replacement.clone()).await?;
                Ok((replacement, snapshot))
            }
            Err(error) => Err(error),
        }
    }

    async fn refresh_verified_snapshot(
        &self,
        authorized: AuthorizedPath,
    ) -> Result<VerifiedFileSnapshot, FileResourceErrorV1> {
        #[cfg(test)]
        self.inner.refresh_scan_count.fetch_add(1, Ordering::AcqRel);
        #[cfg(test)]
        if let Some(error) = self.inner.forced_refresh_error.lock().await.take() {
            return Err(error);
        }
        verified_snapshot(authorized, self.inner.limits.clone()).await
    }

    async fn publish_refresh_failure(
        &self,
        resource_id: &str,
        incarnation_id: Uuid,
        generation: u64,
        expected_subscriptions: &[String],
        failure: &FileResourceErrorV1,
    ) {
        let (event, reschedule) = {
            let mut entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get_mut(resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id || entry.debounce_generation != generation {
                return;
            }
            if !same_subscriptions(entry, expected_subscriptions) {
                (None, true)
            } else if entry.descriptor.unavailable_reason.as_deref() == Some(failure.code()) {
                (None, false)
            } else {
                let mut descriptor = entry.descriptor.clone();
                descriptor.capabilities.preview = false;
                descriptor.capabilities.changes = false;
                descriptor.capabilities.draft = false;
                descriptor.capabilities.stream = false;
                descriptor.unavailable_reason = Some(failure.code().to_string());
                entry.revision = entry.revision.saturating_add(1);
                entry.descriptor = descriptor.clone();
                (
                    Some(FileResourceEventV1 {
                        schema: 1,
                        resource_id: resource_id.to_string(),
                        revision: entry.revision,
                        descriptor,
                    }),
                    false,
                )
            }
        };
        if reschedule {
            self.schedule_refresh_for_incarnation(resource_id.to_string(), incarnation_id);
        } else if let Some(event) = event {
            self.emit(event);
        }
    }

    fn emit(&self, event: FileResourceEventV1) {
        let _ = self.inner.events.send(event.clone());
        let app_handle = self
            .inner
            .app_handle
            .read()
            .map(|handle| handle.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        if let Some(app_handle) = app_handle {
            let _ = app_handle.emit(FILE_RESOURCE_REVISION_EVENT, event);
        }
    }

    pub async fn close(&self, subscription_id: &str) -> Result<(), FileResourceErrorV1> {
        let resource_id = self
            .inner
            .subscription_resources
            .lock()
            .await
            .get(subscription_id)
            .cloned();
        let Some(resource_id) = resource_id else {
            return Ok(());
        };
        let operation = self
            .inner
            .entries
            .lock()
            .await
            .get(&resource_id)
            .map(|entry| entry.operation.clone());
        let _operation = match operation {
            Some(operation) => Some(operation.lock_owned().await),
            None => None,
        };
        let removed_mapping = {
            let mut subscriptions = self.inner.subscription_resources.lock().await;
            if subscriptions
                .get(subscription_id)
                .is_some_and(|current| current == &resource_id)
            {
                subscriptions.remove(subscription_id);
                true
            } else {
                false
            }
        };
        if !removed_mapping {
            return Ok(());
        }
        let (removed_access, remaining_incarnation_id) = {
            let mut entries = self.inner.entries.lock().await;
            let removed_access = entries
                .get_mut(&resource_id)
                .and_then(|entry| entry.subscribers.remove(subscription_id));
            let entry_became_empty = entries
                .get(&resource_id)
                .is_some_and(|entry| entry.subscribers.is_empty());
            let remaining_incarnation_id = entries
                .get(&resource_id)
                .filter(|entry| !entry.subscribers.is_empty())
                .map(|entry| entry.incarnation_id);
            if entry_became_empty {
                entries.remove(&resource_id);
            }
            (removed_access, remaining_incarnation_id)
        };
        if let Some(FileSubscriptionAccess {
            claim: FileAccessClaim::User { capability_id },
            ..
        }) = removed_access
        {
            if let Some(grant) = self
                .inner
                .user_file_grants
                .lock()
                .await
                .get_mut(&capability_id)
            {
                grant.active_subscriptions = grant.active_subscriptions.saturating_sub(1);
            }
        }
        if let Some(incarnation_id) = remaining_incarnation_id {
            self.schedule_refresh_for_incarnation(resource_id.clone(), incarnation_id);
        }
        self.inner
            .read_tickets
            .lock()
            .await
            .retain(|_, ticket| ticket.subscription_id != subscription_id);
        self.inner
            .renderer_leases
            .lock()
            .await
            .retain(|_, lease| lease.subscription_id != subscription_id);
        Ok(())
    }

    pub async fn snapshot(
        &self,
        resource_id: &str,
    ) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
        let entries = self.inner.entries.lock().await;
        let entry = entries
            .get(resource_id)
            .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
        let subscription_id = entry
            .subscribers
            .keys()
            .min()
            .cloned()
            .ok_or_else(|| error("resource_not_found", "file resource has no subscriber"))?;
        Ok(FileResourceSnapshotV1 {
            resource_id: resource_id.to_string(),
            subscription_id,
            revision: entry.revision,
            descriptor: entry.descriptor.clone(),
        })
    }

    pub async fn authorization_agent_id(
        &self,
        resource_id: &str,
        subscription_id: &str,
    ) -> Result<Option<String>, FileResourceErrorV1> {
        let entries = self.inner.entries.lock().await;
        let access = entries
            .get(resource_id)
            .and_then(|entry| entry.subscribers.get(subscription_id))
            .ok_or_else(|| {
                error(
                    "unauthorized_resource",
                    "subscription does not grant the requested resource",
                )
            })?;
        Ok(match &access.claim {
            FileAccessClaim::Agent { agent_id } => Some(agent_id.clone()),
            FileAccessClaim::User { .. } => None,
        })
    }

    async fn validated_authorized(
        &self,
        resource_id: &str,
        subscription_id: &str,
        revision: u64,
        current_agent_config: Option<&AgentConfig>,
    ) -> Result<(AuthorizedPath, FileContentDescriptorV1, FileRevisionToken), FileResourceErrorV1>
    {
        let (current_revision, authorized, expected, revision_token) = self
            .validated_authorized_current(resource_id, subscription_id, current_agent_config)
            .await?;
        if current_revision != revision {
            return Err(error(
                "stale_revision",
                "requested revision is no longer current",
            ));
        }
        Ok((authorized, expected, revision_token))
    }

    async fn validated_authorized_current(
        &self,
        resource_id: &str,
        subscription_id: &str,
        current_agent_config: Option<&AgentConfig>,
    ) -> Result<
        (
            u64,
            AuthorizedPath,
            FileContentDescriptorV1,
            FileRevisionToken,
        ),
        FileResourceErrorV1,
    > {
        let (revision, access, expected, revision_token) = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            let access = entry
                .subscribers
                .get(subscription_id)
                .cloned()
                .ok_or_else(|| {
                    error(
                        "unauthorized_resource",
                        "subscription does not grant the requested resource",
                    )
                })?;
            (
                entry.revision,
                access,
                entry.descriptor.clone(),
                entry.revision_token.clone(),
            )
        };

        let authorized = match &access.claim {
            FileAccessClaim::Agent { agent_id } => {
                let config = current_agent_config.ok_or_else(|| {
                    error(
                        "unauthorized_path",
                        "current agent authorization is unavailable",
                    )
                })?;
                if &config.session_id != agent_id {
                    return Err(error(
                        "unauthorized_path",
                        "current agent authorization does not match the subscription",
                    ));
                }
                access.authorized.reauthorize_same_target()?;
                let current = AuthorizedRootService::from_agent_config(config)?
                    .authorize_existing_file(access.authorized.requested_path())?;
                if current.canonical_path != Path::new(&expected.canonical_path) {
                    return Err(error(
                        "unauthorized_path",
                        "current agent authorization resolves to another file",
                    ));
                }
                current
            }
            FileAccessClaim::User { capability_id } => {
                let grant = self
                    .inner
                    .user_file_grants
                    .lock()
                    .await
                    .get(capability_id)
                    .cloned()
                    .ok_or_else(|| {
                        error("unauthorized_path", "user file capability was revoked")
                    })?;
                if grant.canonical_path != expected.canonical_path {
                    return Err(error(
                        "unauthorized_path",
                        "user file capability does not match the resource",
                    ));
                }
                let current = access.authorized.reauthorize_same_target()?;
                if current.canonical_path != Path::new(&expected.canonical_path) {
                    return Err(error(
                        "unauthorized_path",
                        "user file subscription resolves to another file",
                    ));
                }
                current
            }
        };
        if let Some(reason) = expected.unavailable_reason.as_deref() {
            return Err(error(
                reason,
                format!("file resource is unavailable at revision {revision}"),
            ));
        }
        Ok((revision, authorized, expected, revision_token))
    }

    async fn validated_save_authorization(
        &self,
        resource_id: &str,
        subscription_id: &str,
    ) -> Result<
        (
            u64,
            AuthorizedPath,
            FileContentDescriptorV1,
            FileRevisionToken,
        ),
        FileResourceErrorV1,
    > {
        let (revision, access, expected, revision_token) = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            let access = entry
                .subscribers
                .get(subscription_id)
                .cloned()
                .ok_or_else(|| {
                    error(
                        "unauthorized_resource",
                        "subscription does not grant the requested resource",
                    )
                })?;
            (
                entry.revision,
                access,
                entry.descriptor.clone(),
                entry.revision_token.clone(),
            )
        };
        let authorized = self.validate_refresh_candidate(&access).await?;
        if authorized.canonical_path != Path::new(&expected.canonical_path) {
            return Err(error(
                "unauthorized_path",
                "current backend authorization resolves to another file",
            ));
        }
        if let Some(reason) = expected.unavailable_reason.as_deref() {
            return Err(error(
                reason,
                format!("file resource is unavailable at revision {revision}"),
            ));
        }
        Ok((revision, authorized, expected, revision_token))
    }

    async fn validate_save_claims_at_commit(
        &self,
        resource_id: &str,
        expected_subscriptions: &[String],
    ) -> Result<(), FileResourceErrorV1> {
        let (expected_canonical_path, candidates) = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            let mut current_subscriptions = entry.subscribers.keys().cloned().collect::<Vec<_>>();
            current_subscriptions.sort();
            if current_subscriptions != expected_subscriptions {
                return Err(error(
                    "unauthorized_resource",
                    "live subscriptions changed during guarded save",
                ));
            }
            (
                entry.descriptor.canonical_path.clone(),
                entry
                    .subscribers
                    .iter()
                    .map(|(subscription_id, access)| FileRefreshCandidate {
                        subscription_id: subscription_id.clone(),
                        access: access.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
        };
        for candidate in candidates {
            let authorized = self.validate_refresh_candidate(&candidate.access).await?;
            if authorized.canonical_path != Path::new(&expected_canonical_path) {
                return Err(error(
                    "unauthorized_path",
                    "commit-time backend authorization resolves to another file",
                ));
            }
        }
        Ok(())
    }

    /// Durably checkpoints one dirty editor buffer under the exact live file
    /// subscription and a compare-and-swap recovery revision. New records
    /// retain the submitted hash-verified editor base even when the authorized
    /// disk head advanced before the first checkpoint. An exact CAS update may
    /// advance the stored base after a guarded Save or accepted rebase.
    #[allow(clippy::too_many_arguments)]
    pub async fn checkpoint_recovery(
        &self,
        recovery_id: Option<&str>,
        expected_recovery_revision: Option<u64>,
        resource_id: &str,
        subscription_id: &str,
        base_content_hash: &str,
        submitted_base: &str,
        resource_key: &str,
        webview_scope: &str,
        buffer: &str,
    ) -> Result<FileRecoveryCheckpointV1, FileResourceErrorV1> {
        validate_submitted_text(submitted_base, &self.inner.limits)?;
        validate_submitted_text(buffer, &self.inner.limits)?;
        let submitted_base_hash = format!("sha256:{:x}", Sha256::digest(submitted_base.as_bytes()));
        if submitted_base_hash != base_content_hash {
            return Err(error(
                "invalid_request",
                "recovery base content does not match its declared hash",
            ));
        }
        if resource_key.trim().is_empty() || webview_scope.trim().is_empty() {
            return Err(error(
                "invalid_request",
                "recovery resource key and webview scope must not be empty",
            ));
        }
        // Creating recovery authority requires one exact live file capability.
        // Updating an already-scoped recovery CAS does not: it can only replace
        // the recovery blobs owned by the same resource key and app webview and
        // never reads or writes the current file.
        let operation = if recovery_id.is_none() {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            if !entry.subscribers.contains_key(subscription_id) {
                return Err(error(
                    "unauthorized_resource",
                    "subscription does not grant the requested resource",
                ));
            }
            Some(entry.operation.clone())
        } else {
            None
        };
        let _operation = match operation.as_ref() {
            Some(operation) => Some(operation.lock().await),
            None => None,
        };
        let descriptor = if recovery_id.is_none() {
            let (_, _, descriptor, _) = self
                .validated_save_authorization(resource_id, subscription_id)
                .await?;
            if file_resource_id(&descriptor.canonical_path) != resource_key
                || resource_id != resource_key
            {
                return Err(error(
                    "unauthorized_resource",
                    "live subscription does not match the recovery resource key",
                ));
            }
            Some(descriptor)
        } else {
            None
        };
        let recovery_root = self.recovery_root()?;
        let _recovery_io = self.inner.recovery_io.lock().await;
        let store_limits = self.recovery_store_limits();
        sweep_recovery_store(&recovery_root, store_limits.orphan_grace_period)?;
        let now = now_epoch_ms();
        let (manifest, base) = match recovery_id {
            Some(recovery_id) => {
                let mut current = load_recovery_manifest(&recovery_root, recovery_id)?;
                authorize_recovery_manifest(&current, resource_key, webview_scope)?;
                if expected_recovery_revision != Some(current.recovery_revision) {
                    return Err(error(
                        "recovery_conflict",
                        "recovery checkpoint revision is no longer current",
                    ));
                }
                if current.base_content_hash != base_content_hash {
                    current.base_content_hash = base_content_hash.to_string();
                    current.base_opaque_revision = Uuid::new_v4().to_string();
                }
                (current, submitted_base.to_string())
            }
            None => {
                if expected_recovery_revision.is_some() {
                    return Err(error(
                        "recovery_conflict",
                        "a new recovery checkpoint has no prior revision",
                    ));
                }
                let descriptor = descriptor.ok_or_else(|| {
                    error(
                        "resource_not_found",
                        "new recovery checkpoint lost its validated file resource",
                    )
                })?;
                let recovery_id = Uuid::new_v4().to_string();
                (
                    FileRecoveryManifestV1 {
                        schema: 1,
                        recovery_id,
                        resource_key: resource_key.to_string(),
                        display_name: descriptor.display_name,
                        extension: descriptor.extension,
                        mime_type: descriptor.mime_type,
                        base_content_hash: base_content_hash.to_string(),
                        base_opaque_revision: Uuid::new_v4().to_string(),
                        base_blob: String::new(),
                        buffer_blob: String::new(),
                        recovery_revision: 0,
                        webview_scope: webview_scope.to_string(),
                        created_at_ms: now,
                        updated_at_ms: now,
                    },
                    submitted_base.to_string(),
                )
            }
        };
        enforce_recovery_admission(
            &recovery_root,
            &manifest.recovery_id,
            &base,
            buffer,
            store_limits,
        )?;
        let fail_before_manifest = {
            #[cfg(test)]
            {
                self.inner
                    .fail_recovery_before_manifest
                    .swap(false, Ordering::AcqRel)
            }
            #[cfg(not(test))]
            {
                false
            }
        };
        let committed = write_recovery_checkpoint(
            &recovery_root,
            manifest,
            &base,
            buffer,
            now,
            fail_before_manifest,
            store_limits.orphan_grace_period,
        )?;
        drop(_recovery_io);
        // Recovery authority and file authority are deliberately independent.
        // Probe the latter only after the CAS is durable so root revocation or
        // subscription closure makes the editor read-only without losing the
        // latest recovery generation.
        let file_authorization_error = match self
            .validated_save_authorization(resource_id, subscription_id)
            .await
        {
            Ok((_, _, descriptor, _))
                if resource_id == resource_key
                    && file_resource_id(&descriptor.canonical_path) == resource_key =>
            {
                None
            }
            Ok(_) => Some(error(
                "unauthorized_resource",
                "live subscription does not match the recovery resource key",
            )),
            Err(error) => Some(error),
        };
        Ok(recovery_checkpoint_metadata(
            &committed,
            file_authorization_error,
        ))
    }

    /// Discovers body-free recovery metadata for one exact stable resource key
    /// and calling WebView scope. Results are newest-first and never confer
    /// current-file authority.
    pub async fn list_recoveries(
        &self,
        resource_key: &str,
        webview_scope: &str,
    ) -> Result<Vec<FileRecoverySummaryV1>, FileResourceErrorV1> {
        if resource_key.trim().is_empty() || webview_scope.trim().is_empty() {
            return Err(error(
                "invalid_request",
                "recovery resource key and webview scope must not be empty",
            ));
        }
        let recovery_root = self.recovery_root()?;
        let _recovery_io = self.inner.recovery_io.lock().await;
        let store_limits = self.recovery_store_limits();
        sweep_recovery_store(&recovery_root, store_limits.orphan_grace_period)?;
        let mut recoveries = Vec::new();
        for recovery_id in recovery_record_ids(&recovery_root)? {
            let Ok(manifest) = load_recovery_manifest(&recovery_root, &recovery_id) else {
                continue;
            };
            if authorize_recovery_manifest(&manifest, resource_key, webview_scope).is_err() {
                continue;
            }
            if validate_recovery_blob_metadata(
                &recovery_root,
                &manifest,
                &manifest.base_blob,
                &self.inner.limits,
            )
            .and_then(|_| {
                validate_recovery_blob_metadata(
                    &recovery_root,
                    &manifest,
                    &manifest.buffer_blob,
                    &self.inner.limits,
                )
            })
            .is_err()
            {
                continue;
            }
            recoveries.push(recovery_summary_metadata(&manifest));
        }
        recoveries.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| right.recovery_id.cmp(&left.recovery_id))
        });
        Ok(recoveries)
    }

    /// Reads only persisted recovery bytes under the exact WebView and stable
    /// resource key. This deliberately performs no filesystem authorization.
    pub async fn get_recovery(
        &self,
        recovery_id: &str,
        resource_key: &str,
        webview_scope: &str,
    ) -> Result<FileRecoveryV1, FileResourceErrorV1> {
        let recovery_root = self.recovery_root()?;
        let _recovery_io = self.inner.recovery_io.lock().await;
        let store_limits = self.recovery_store_limits();
        sweep_recovery_store(&recovery_root, store_limits.orphan_grace_period)?;
        let manifest = load_recovery_manifest(&recovery_root, recovery_id)?;
        authorize_recovery_manifest(&manifest, resource_key, webview_scope)?;
        let base = read_recovery_blob(
            &recovery_root,
            &manifest,
            &manifest.base_blob,
            &self.inner.limits,
        )?;
        let buffer = read_recovery_blob(
            &recovery_root,
            &manifest,
            &manifest.buffer_blob,
            &self.inner.limits,
        )?;
        Ok(FileRecoveryV1 {
            schema: 1,
            recovery_id: manifest.recovery_id,
            resource_key: manifest.resource_key,
            display_name: manifest.display_name,
            extension: manifest.extension,
            mime_type: manifest.mime_type,
            base_content_hash: manifest.base_content_hash,
            base_opaque_revision: manifest.base_opaque_revision,
            recovery_revision: manifest.recovery_revision,
            base,
            buffer,
            created_at_ms: manifest.created_at_ms,
            updated_at_ms: manifest.updated_at_ms,
        })
    }

    /// Discards one exact scoped recovery generation after a recovery CAS
    /// check. Discard never opens or mutates the underlying file resource.
    pub async fn discard_recovery(
        &self,
        recovery_id: &str,
        expected_recovery_revision: u64,
        resource_key: &str,
        webview_scope: &str,
    ) -> Result<(), FileResourceErrorV1> {
        let recovery_root = self.recovery_root()?;
        let _recovery_io = self.inner.recovery_io.lock().await;
        let store_limits = self.recovery_store_limits();
        sweep_recovery_store(&recovery_root, store_limits.orphan_grace_period)?;
        let manifest = load_recovery_manifest(&recovery_root, recovery_id)?;
        authorize_recovery_manifest(&manifest, resource_key, webview_scope)?;
        if manifest.recovery_revision != expected_recovery_revision {
            return Err(error(
                "recovery_conflict",
                "recovery checkpoint revision is no longer current",
            ));
        }
        remove_recovery_record(&recovery_root, recovery_id)
    }

    /// Three-way merges persisted base/editor bytes with the newly scanned
    /// disk head from one exact, currently authorized live subscription.
    #[allow(clippy::too_many_arguments)]
    pub async fn merge_recovery(
        &self,
        recovery_id: &str,
        expected_recovery_revision: u64,
        resource_key: &str,
        webview_scope: &str,
        resource_id: &str,
        subscription_id: &str,
    ) -> Result<FileRecoveryMergeResultV1, FileResourceErrorV1> {
        let operation = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            if !entry.subscribers.contains_key(subscription_id) {
                return Err(error(
                    "unauthorized_resource",
                    "subscription does not grant the requested resource",
                ));
            }
            entry.operation.clone()
        };
        let _operation = operation.lock().await;
        let (_, authorized, descriptor, _) = self
            .validated_save_authorization(resource_id, subscription_id)
            .await?;
        if file_resource_id(&descriptor.canonical_path) != resource_key
            || resource_id != resource_key
        {
            return Err(error(
                "unauthorized_resource",
                "live subscription does not match the recovery resource key",
            ));
        }

        let recovery_root = self.recovery_root()?;
        let (manifest, base, buffer) = {
            let _recovery_io = self.inner.recovery_io.lock().await;
            let store_limits = self.recovery_store_limits();
            sweep_recovery_store(&recovery_root, store_limits.orphan_grace_period)?;
            let manifest = load_recovery_manifest(&recovery_root, recovery_id)?;
            authorize_recovery_manifest(&manifest, resource_key, webview_scope)?;
            if manifest.recovery_revision != expected_recovery_revision {
                return Err(error(
                    "recovery_conflict",
                    "recovery checkpoint revision is no longer current",
                ));
            }
            let base = read_recovery_blob(
                &recovery_root,
                &manifest,
                &manifest.base_blob,
                &self.inner.limits,
            )?;
            let buffer = read_recovery_blob(
                &recovery_root,
                &manifest,
                &manifest.buffer_blob,
                &self.inner.limits,
            )?;
            validate_recovery_diff_side(&base, &self.inner.limits)?;
            validate_recovery_diff_side(&buffer, &self.inner.limits)?;
            (manifest, base, buffer)
        };

        let (authorized, snapshot) = self.refresh_from_authorization(authorized).await?;
        let (current_descriptor, current_token) = snapshot.into_parts();
        if !matches!(
            current_descriptor.renderer_kind,
            FileRendererKind::Text | FileRendererKind::Markdown
        ) || current_descriptor.encoding.as_deref() != Some("utf-8")
            || !current_descriptor.capabilities.draft
        {
            return Err(error(
                "unsupported_content",
                "current file is not an editable UTF-8 text model",
            ));
        }
        if !current_descriptor.capabilities.changes {
            return Err(error(
                "file_too_large",
                "current file exceeds the per-side recovery merge limits",
            ));
        }
        let maximum_length_bytes = self.inner.limits.monaco_max_size_bytes;
        let authorized_for_read = authorized.clone();
        let token_for_read = current_token.clone();
        let current = tauri::async_runtime::spawn_blocking(move || {
            authorized_for_read.read_verified_text(&token_for_read, maximum_length_bytes)
        })
        .await
        .map_err(join_error)??;
        validate_recovery_diff_side(&current, &self.inner.limits)?;

        let (current_revision, event, user_grant_update) = {
            let mut entries = self.inner.entries.lock().await;
            let entry = entries
                .get_mut(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            let access = entry.subscribers.get_mut(subscription_id).ok_or_else(|| {
                error(
                    "unauthorized_resource",
                    "subscription does not grant the requested resource",
                )
            })?;
            access.authorized = authorized.clone();
            let user_grant_update = match &access.claim {
                FileAccessClaim::User { capability_id } => {
                    Some((capability_id.clone(), authorized))
                }
                FileAccessClaim::Agent { .. } => None,
            };
            entry.revision_token = current_token;
            let changed = entry.descriptor.content_hash != current_descriptor.content_hash
                || entry.descriptor.unavailable_reason != current_descriptor.unavailable_reason;
            entry.descriptor = current_descriptor.clone();
            let event = changed.then(|| {
                entry.revision = entry.revision.saturating_add(1);
                FileResourceEventV1 {
                    schema: 1,
                    resource_id: resource_id.to_string(),
                    revision: entry.revision,
                    descriptor: current_descriptor.clone(),
                }
            });
            (entry.revision, event, user_grant_update)
        };
        if let Some((capability_id, authorized)) = user_grant_update {
            if let Some(grant) = self
                .inner
                .user_file_grants
                .lock()
                .await
                .get_mut(&capability_id)
            {
                grant.authorized = authorized;
                grant.last_used_at = Instant::now();
            }
        }
        if let Some(event) = event {
            self.emit(event);
        }

        let current_content_hash = current_descriptor.content_hash;
        let disk_changed = current_content_hash != manifest.base_content_hash;
        let merged = if disk_changed {
            diffy::merge(&base, &buffer, &current)
        } else {
            Ok(buffer)
        };
        finalize_recovery_merge(
            merged,
            manifest.recovery_revision,
            current_revision,
            current_content_hash,
            disk_changed,
            &self.inner.limits,
        )
    }

    /// Performs a guarded save and then best-effort cleanup of one exact
    /// scoped recovery generation. A committed save is never reported as a
    /// failure solely because recovery cleanup raced or became unavailable.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_text_with_recovery_cleanup(
        &self,
        resource_id: &str,
        subscription_id: &str,
        expected_revision: u64,
        buffer_base_hash: &str,
        text: &str,
        recovery_cleanup: Option<&FileRecoveryCleanupV1>,
        webview_scope: &str,
    ) -> Result<FileResourceSaveResultV1, FileResourceErrorV1> {
        if recovery_cleanup.is_some() && webview_scope.trim().is_empty() {
            return Err(error(
                "invalid_request",
                "recovery cleanup requires a calling webview scope",
            ));
        }
        let result = self
            .save_text(
                resource_id,
                subscription_id,
                expected_revision,
                buffer_base_hash,
                text,
            )
            .await?;
        if matches!(
            result,
            FileResourceSaveResultV1::Saved { .. } | FileResourceSaveResultV1::Unchanged { .. }
        ) {
            if let Some(cleanup) = recovery_cleanup {
                if let Err(failure) = self
                    .discard_recovery(
                        &cleanup.recovery_id,
                        cleanup.expected_recovery_revision,
                        resource_id,
                        webview_scope,
                    )
                    .await
                {
                    crate::manager::log_debug(&format!(
                        "[Wardian] saved file but left recovery {} for conservative cleanup: {}",
                        cleanup.recovery_id, failure
                    ));
                }
            }
        }
        Ok(result)
    }

    fn recovery_root(&self) -> Result<PathBuf, FileResourceErrorV1> {
        self.inner
            .recovery_root
            .read()
            .map(|root| root.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
            .ok_or_else(|| {
                error(
                    "runtime_unavailable",
                    "Wardian recovery home is unavailable",
                )
            })
    }

    fn recovery_store_limits(&self) -> FileRecoveryStoreLimits {
        self.inner
            .recovery_store_limits
            .read()
            .map(|limits| *limits)
            .unwrap_or_else(|poisoned| *poisoned.into_inner())
    }

    /// Saves UTF-8 text through one exact live subscription and its private
    /// retained-handle revision capability.
    ///
    /// Save, watcher refresh, and close operations share the resource's
    /// operation mutex. Optimistic mismatches are returned as metadata-only
    /// `stale_conflict` values after current authorization is revalidated.
    pub async fn save_text(
        &self,
        resource_id: &str,
        subscription_id: &str,
        expected_revision: u64,
        buffer_base_hash: &str,
        text: &str,
    ) -> Result<FileResourceSaveResultV1, FileResourceErrorV1> {
        let operation = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            if !entry.subscribers.contains_key(subscription_id) {
                return Err(error(
                    "unauthorized_resource",
                    "subscription does not grant the requested resource",
                ));
            }
            entry.operation.clone()
        };
        let _operation = operation.lock().await;
        let (current_revision, authorized, descriptor, revision_token) = self
            .validated_save_authorization(resource_id, subscription_id)
            .await?;
        if !matches!(
            descriptor.renderer_kind,
            FileRendererKind::Text | FileRendererKind::Markdown
        ) || descriptor.encoding.as_deref() != Some("utf-8")
            || !descriptor.capabilities.draft
        {
            return Err(error(
                "unsupported_content",
                "file resource is not an editable UTF-8 text model",
            ));
        }
        if expected_revision != current_revision
            || buffer_base_hash != descriptor.content_hash.as_str()
        {
            return Ok(FileResourceSaveResultV1::StaleConflict {
                revision: current_revision,
                content_hash: descriptor.content_hash,
            });
        }

        let candidates = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            entry
                .subscribers
                .iter()
                .map(|(candidate_subscription_id, access)| FileRefreshCandidate {
                    subscription_id: candidate_subscription_id.clone(),
                    access: access.clone(),
                })
                .collect::<Vec<_>>()
        };
        let mut prevalidated_authorizations = HashMap::with_capacity(candidates.len());
        for candidate in candidates {
            let candidate_authorized = if candidate.subscription_id == subscription_id {
                authorized.clone()
            } else {
                self.validate_refresh_candidate(&candidate.access).await?
            };
            prevalidated_authorizations.insert(candidate.subscription_id, candidate_authorized);
        }
        #[cfg(test)]
        {
            let hook = self.inner.save_after_validation_hook.lock().await.clone();
            if let Some(hook) = hook {
                hook.validation_reached.wait().await;
                hook.resume_save.wait().await;
            }
        }

        let limits = self.inner.limits.clone();
        let expected_hash = buffer_base_hash.to_string();
        let submitted = text.to_string();
        let authorized_for_refresh = authorized.clone();
        let mut expected_subscriptions = prevalidated_authorizations
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        expected_subscriptions.sort();
        let commit_runtime = self.clone();
        let commit_resource_id = resource_id.to_string();
        let runtime_handle = tokio::runtime::Handle::current();
        let write = tauri::async_runtime::spawn_blocking(move || {
            authorized.guarded_atomic_replace_text_with_commit_check(
                &revision_token,
                &expected_hash,
                &submitted,
                &limits,
                move || {
                    runtime_handle.block_on(commit_runtime.validate_save_claims_at_commit(
                        &commit_resource_id,
                        &expected_subscriptions,
                    ))
                },
            )
        })
        .await
        .map_err(join_error)?;
        let write = match write {
            Ok(write) => write,
            Err(failure) if failure.code() == "stale_revision" => {
                let (authorized, snapshot) = self
                    .refresh_from_authorization(authorized_for_refresh)
                    .await?;
                let (descriptor, revision_token) = snapshot.into_parts();
                let (revision, content_hash, event, user_grant_update) = {
                    let mut entries = self.inner.entries.lock().await;
                    let entry = entries
                        .get_mut(resource_id)
                        .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
                    let access = entry.subscribers.get_mut(subscription_id).ok_or_else(|| {
                        error(
                            "unauthorized_resource",
                            "subscription does not grant the requested resource",
                        )
                    })?;
                    access.authorized = authorized.clone();
                    let user_grant_update = match &access.claim {
                        FileAccessClaim::User { capability_id } => {
                            Some((capability_id.clone(), authorized))
                        }
                        FileAccessClaim::Agent { .. } => None,
                    };
                    entry.revision_token = revision_token;
                    let changed = entry.descriptor.content_hash != descriptor.content_hash
                        || entry.descriptor.unavailable_reason != descriptor.unavailable_reason;
                    entry.descriptor = descriptor.clone();
                    let event = changed.then(|| {
                        entry.revision = entry.revision.saturating_add(1);
                        FileResourceEventV1 {
                            schema: 1,
                            resource_id: resource_id.to_string(),
                            revision: entry.revision,
                            descriptor: descriptor.clone(),
                        }
                    });
                    (
                        entry.revision,
                        descriptor.content_hash,
                        event,
                        user_grant_update,
                    )
                };
                if let Some((capability_id, authorized)) = user_grant_update {
                    if let Some(grant) = self
                        .inner
                        .user_file_grants
                        .lock()
                        .await
                        .get_mut(&capability_id)
                    {
                        grant.authorized = authorized;
                        grant.last_used_at = Instant::now();
                    }
                }
                if let Some(event) = event {
                    self.emit(event);
                }
                return Ok(FileResourceSaveResultV1::StaleConflict {
                    revision,
                    content_hash,
                });
            }
            Err(failure) => return Err(failure),
        };
        let submitted_text_is_current = write.submitted_text_is_current();
        let rebound_authorizations = prevalidated_authorizations
            .into_iter()
            .map(|(candidate_subscription_id, previous)| {
                write
                    .rebind_authorization(&previous)
                    .map(|authorized| (candidate_subscription_id, authorized))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let (changed, _authorized, snapshot) = write.into_parts();
        let (descriptor, revision_token) = snapshot.into_parts();
        let (revision, content_hash, event, user_grant_updates) = {
            let mut entries = self.inner.entries.lock().await;
            let entry = entries
                .get_mut(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            for (candidate_subscription_id, authorized) in &rebound_authorizations {
                let access = entry
                    .subscribers
                    .get_mut(candidate_subscription_id)
                    .ok_or_else(|| {
                        error(
                            "unauthorized_resource",
                            "live subscription changed during guarded save",
                        )
                    })?;
                access.authorized = authorized.clone();
            }
            let user_grant_updates = rebound_authorizations
                .iter()
                .filter_map(|(candidate_subscription_id, authorized)| {
                    let access = entry.subscribers.get(candidate_subscription_id)?;
                    match &access.claim {
                        FileAccessClaim::User { capability_id } => {
                            Some((capability_id.clone(), authorized.clone()))
                        }
                        FileAccessClaim::Agent { .. } => None,
                    }
                })
                .collect::<Vec<_>>();
            entry.revision_token = revision_token;
            let descriptor_changed = entry.descriptor.content_hash != descriptor.content_hash
                || entry.descriptor.unavailable_reason != descriptor.unavailable_reason;
            entry.descriptor = descriptor.clone();
            let event = descriptor_changed.then(|| {
                entry.revision = entry.revision.saturating_add(1);
                FileResourceEventV1 {
                    schema: 1,
                    resource_id: resource_id.to_string(),
                    revision: entry.revision,
                    descriptor: descriptor.clone(),
                }
            });
            (
                entry.revision,
                descriptor.content_hash,
                event,
                user_grant_updates,
            )
        };
        if !user_grant_updates.is_empty() {
            let now = Instant::now();
            let mut grants = self.inner.user_file_grants.lock().await;
            for (capability_id, authorized) in user_grant_updates {
                if let Some(grant) = grants.get_mut(&capability_id) {
                    grant.authorized = authorized;
                    grant.last_used_at = now;
                }
            }
        }
        if let Some(event) = event {
            self.emit(event);
        }
        Ok(if !submitted_text_is_current {
            FileResourceSaveResultV1::StaleConflict {
                revision,
                content_hash,
            }
        } else if changed {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            }
        } else {
            FileResourceSaveResultV1::Unchanged {
                revision,
                content_hash,
            }
        })
    }

    pub async fn read_text(
        &self,
        resource_id: &str,
        subscription_id: &str,
        revision: u64,
        current_agent_config: Option<&AgentConfig>,
    ) -> Result<FileResourceTextV1, FileResourceErrorV1> {
        let (authorized, descriptor, revision_token) = self
            .validated_authorized(resource_id, subscription_id, revision, current_agent_config)
            .await?;
        if !matches!(
            descriptor.renderer_kind,
            FileRendererKind::Text | FileRendererKind::Markdown
        ) || descriptor.encoding.as_deref() != Some("utf-8")
        {
            return Err(error(
                "unsupported_content",
                "file resource is not validated UTF-8 text",
            ));
        }
        let line_count = descriptor.line_count.unwrap_or(u64::MAX);
        if !self
            .inner
            .limits
            .allows_monaco(descriptor.size_bytes, line_count)
        {
            return Err(error(
                "file_too_large",
                "text resource exceeds the complete model limits",
            ));
        }
        let maximum_length_bytes = self.inner.limits.monaco_max_size_bytes;
        let text = tauri::async_runtime::spawn_blocking(move || {
            authorized.read_verified_text(&revision_token, maximum_length_bytes)
        })
        .await
        .map_err(join_error)??;
        Ok(FileResourceTextV1 {
            schema: 1,
            resource_id: resource_id.to_string(),
            revision,
            text,
        })
    }

    pub async fn issue_ticket(
        &self,
        resource_id: &str,
        subscription_id: &str,
        revision: u64,
        current_agent_config: Option<&AgentConfig>,
        renderer_lease_id: &str,
    ) -> Result<FileResourceTicketV1, FileResourceErrorV1> {
        self.issue_ticket_for_webview(
            resource_id,
            subscription_id,
            revision,
            current_agent_config,
            renderer_lease_id,
            None,
        )
        .await
    }

    pub async fn issue_ticket_for_webview(
        &self,
        resource_id: &str,
        subscription_id: &str,
        revision: u64,
        current_agent_config: Option<&AgentConfig>,
        renderer_lease_id: &str,
        webview_label: Option<&str>,
    ) -> Result<FileResourceTicketV1, FileResourceErrorV1> {
        if renderer_lease_id.trim().is_empty() {
            return Err(error(
                "invalid_request",
                "renderer lease id must not be empty",
            ));
        }
        let (authorized, descriptor, revision_token) = self
            .validated_authorized(resource_id, subscription_id, revision, current_agent_config)
            .await?;
        #[cfg(test)]
        {
            let hook = self
                .inner
                .issue_ticket_after_validation_hook
                .lock()
                .await
                .clone();
            if let Some(hook) = hook {
                hook.validation_reached.wait().await;
                hook.resume_publication.wait().await;
            }
        }
        match descriptor.renderer_kind {
            FileRendererKind::Image | FileRendererKind::Pdf if descriptor.capabilities.stream => {}
            _ => {
                return Err(error(
                    "unsupported_content",
                    "resource is not an image or PDF stream",
                ));
            }
        };
        self.remove_expired_tickets().await;
        let snapshot = self
            .create_ticket_snapshot(authorized, revision_token, descriptor.size_bytes)
            .await?;
        let ticket_id = Uuid::new_v4().to_string();
        let issuance_id = Uuid::new_v4();
        let expires_at = Instant::now() + self.inner.ticket_ttl;
        let renderer_lease = RendererLeaseKey {
            webview_label: webview_label.map(str::to_string),
            renderer_lease_id: renderer_lease_id.to_string(),
        };
        let publication = self.inner.ticket_publication.lock().await;
        {
            let mut leases = self.inner.renderer_leases.lock().await;
            if let Some(existing) = leases.get(&renderer_lease) {
                if existing.subscription_id != subscription_id
                    && existing.expires_at > Instant::now()
                {
                    return Err(error(
                        "unauthorized_ticket",
                        "renderer lease is already bound to another subscription",
                    ));
                }
            }
            leases.insert(
                renderer_lease.clone(),
                RendererLease {
                    issuance_id,
                    subscription_id: subscription_id.to_string(),
                    expires_at,
                },
            );
        }
        #[cfg(test)]
        {
            let hook = self.inner.ticket_publication_hook.lock().await.clone();
            if let Some(hook) = hook {
                if hook.pause_once.swap(false, Ordering::AcqRel) {
                    hook.lease_published.wait().await;
                    hook.resume_publication.wait().await;
                }
            }
        }
        let expires_at_ms = now_epoch_ms().saturating_add(
            self.inner
                .ticket_ttl
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        let ticket = FileReadTicket {
            issuance_id,
            webview_label: webview_label.map(str::to_string),
            renderer_lease: renderer_lease.clone(),
            subscription_id: subscription_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot,
            size_bytes: descriptor.size_bytes,
            mime_type: descriptor.mime_type,
            expires_at,
        };
        {
            let mut tickets = self.inner.read_tickets.lock().await;
            tickets.retain(|_, existing| existing.renderer_lease != renderer_lease);
            tickets.insert(ticket_id.clone(), ticket.clone());
        }
        if let Err(error) = self.ensure_ticket_lease_active(&ticket).await {
            self.rollback_ticket_publication(&ticket_id, &ticket).await;
            return Err(error);
        }
        drop(publication);
        self.schedule_ticket_expiry(
            ticket_id.clone(),
            issuance_id,
            ticket.renderer_lease.clone(),
            expires_at,
        );
        Ok(FileResourceTicketV1 {
            schema: 1,
            ticket_id: ticket_id.clone(),
            url: format!("wardian-resource://localhost/{ticket_id}"),
            resource_id: resource_id.to_string(),
            revision,
            renderer_lease_id: renderer_lease_id.to_string(),
            expires_at_ms,
        })
    }

    async fn create_ticket_snapshot(
        &self,
        authorized: AuthorizedPath,
        revision_token: FileRevisionToken,
        size_bytes: u64,
    ) -> Result<Arc<ImmutableTicketSnapshot>, FileResourceErrorV1> {
        let reservation = self.reserve_ticket_snapshot(size_bytes)?;
        tauri::async_runtime::spawn_blocking(move || {
            let mut file = tempfile::tempfile().map_err(|cause| {
                error(
                    "runtime_unavailable",
                    format!("cannot create immutable ticket snapshot: {cause}"),
                )
            })?;
            let copied = authorized.copy_verified_revision_to(&revision_token, &mut file)?;
            if copied != size_bytes {
                return Err(error(
                    "stale_revision",
                    "immutable ticket snapshot length does not match its descriptor",
                ));
            }
            file.seek(SeekFrom::Start(0)).map_err(|cause| {
                error(
                    "runtime_unavailable",
                    format!("cannot rewind immutable ticket snapshot: {cause}"),
                )
            })?;
            Ok(reservation.commit(file))
        })
        .await
        .map_err(join_error)?
    }

    fn reserve_ticket_snapshot(
        &self,
        size_bytes: u64,
    ) -> Result<TicketSnapshotReservation, FileResourceErrorV1> {
        // The accounting floor bounds both anonymous-file bytes and per-ticket
        // metadata/handle growth. Under the 1 GiB default it admits at most
        // 256 tiny tickets, while large PDFs are charged at their exact size.
        let reserved_bytes = size_bytes.max(MIN_TICKET_SNAPSHOT_RESERVATION_BYTES);
        let usage = &self.inner.ticket_snapshot_usage;
        let mut current = usage.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(reserved_bytes) else {
                return Err(error(
                    "ticket_capacity_exceeded",
                    "renderer ticket snapshot budget is exhausted",
                ));
            };
            if next > self.inner.max_ticket_snapshot_bytes {
                return Err(error(
                    "ticket_capacity_exceeded",
                    "renderer ticket snapshot budget is exhausted",
                ));
            }
            match usage.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    return Ok(TicketSnapshotReservation {
                        usage: usage.clone(),
                        size_bytes,
                        reserved_bytes,
                        committed: false,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn schedule_ticket_expiry(
        &self,
        ticket_id: String,
        issuance_id: Uuid,
        renderer_lease: RendererLeaseKey,
        expires_at: Instant,
    ) {
        let weak = Arc::downgrade(&self.inner);
        tauri::async_runtime::spawn(async move {
            loop {
                let remaining = expires_at.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                tokio::time::sleep(remaining).await;
            }
            let Some(inner) = weak.upgrade() else {
                return;
            };
            FileResourceRuntime { inner }
                .expire_ticket_issuance(&ticket_id, issuance_id, &renderer_lease, expires_at)
                .await;
        });
    }

    async fn expire_ticket_issuance(
        &self,
        ticket_id: &str,
        issuance_id: Uuid,
        renderer_lease: &RendererLeaseKey,
        expires_at: Instant,
    ) {
        if expires_at > Instant::now() {
            return;
        }
        {
            let mut tickets = self.inner.read_tickets.lock().await;
            if tickets
                .get(ticket_id)
                .is_some_and(|ticket| ticket.issuance_id == issuance_id)
            {
                tickets.remove(ticket_id);
            }
        }
        let mut leases = self.inner.renderer_leases.lock().await;
        if leases
            .get(renderer_lease)
            .is_some_and(|lease| lease.issuance_id == issuance_id)
        {
            leases.remove(renderer_lease);
        }
    }

    /// Releases one renderer-scoped stream capability without closing the
    /// file subscription shared by other panes and renderers.
    pub async fn close_renderer_lease(
        &self,
        resource_id: &str,
        subscription_id: &str,
        renderer_lease_id: &str,
        webview_label: Option<&str>,
    ) -> Result<(), FileResourceErrorV1> {
        if renderer_lease_id.trim().is_empty() {
            return Err(error(
                "invalid_request",
                "renderer lease id must not be empty",
            ));
        }
        let key = RendererLeaseKey {
            webview_label: webview_label.map(str::to_string),
            renderer_lease_id: renderer_lease_id.to_string(),
        };
        let issuance_id = {
            let leases = self.inner.renderer_leases.lock().await;
            let Some(lease) = leases.get(&key) else {
                return Ok(());
            };
            if lease.subscription_id != subscription_id {
                return Err(error(
                    "unauthorized_ticket",
                    "renderer lease belongs to another file subscription",
                ));
            }
            lease.issuance_id
        };
        let subscription_matches = self
            .inner
            .subscription_resources
            .lock()
            .await
            .get(subscription_id)
            .is_some_and(|current| current == resource_id);
        if !subscription_matches {
            // Closing the resource concurrently already revokes the lease.
            let lease_still_exists = self
                .inner
                .renderer_leases
                .lock()
                .await
                .get(&key)
                .is_some_and(|lease| lease.issuance_id == issuance_id);
            if lease_still_exists {
                return Err(error(
                    "invalid_ticket",
                    "renderer lease file subscription is no longer active",
                ));
            }
            return Ok(());
        }
        let removed = {
            let mut leases = self.inner.renderer_leases.lock().await;
            if leases
                .get(&key)
                .is_some_and(|lease| lease.issuance_id == issuance_id)
            {
                leases.remove(&key);
                true
            } else {
                false
            }
        };
        if removed {
            self.inner.read_tickets.lock().await.retain(|_, ticket| {
                ticket.renderer_lease != key || ticket.issuance_id != issuance_id
            });
        }
        Ok(())
    }

    pub async fn read_ticket_range(
        &self,
        ticket_id: &str,
        range_header: Option<&str>,
    ) -> Result<FileResourceRangeRead, FileResourceErrorV1> {
        self.read_ticket_range_for_webview(ticket_id, range_header, None)
            .await
    }

    pub async fn read_ticket_range_for_webview(
        &self,
        ticket_id: &str,
        range_header: Option<&str>,
        webview_label: Option<&str>,
    ) -> Result<FileResourceRangeRead, FileResourceErrorV1> {
        let ticket = self.validated_ticket(ticket_id, webview_label).await?;
        let range = parse_byte_range(range_header, ticket.size_bytes)?;
        let partial = range_header.is_some();
        let snapshot = ticket.snapshot.clone();
        let bytes =
            tauri::async_runtime::spawn_blocking(move || snapshot.read_range(range.0, range.1))
                .await
                .map_err(join_error)??;
        self.ensure_ticket_lease_active(&ticket).await?;
        Ok(FileResourceRangeRead {
            bytes,
            mime_type: ticket.mime_type,
            start: range.0,
            end: range.1,
            total_size: ticket.size_bytes,
            partial,
        })
    }

    pub async fn verify_ticket_range_for_webview(
        &self,
        ticket_id: &str,
        range_header: Option<&str>,
        webview_label: Option<&str>,
    ) -> Result<FileResourceRangeRead, FileResourceErrorV1> {
        let ticket = self.validated_ticket(ticket_id, webview_label).await?;
        let range = parse_byte_range(range_header, ticket.size_bytes)?;
        let partial = range_header.is_some();
        self.ensure_ticket_lease_active(&ticket).await?;
        Ok(FileResourceRangeRead {
            bytes: Vec::new(),
            mime_type: ticket.mime_type,
            start: range.0,
            end: range.1,
            total_size: ticket.size_bytes,
            partial,
        })
    }

    pub async fn ticket_size_for_webview(
        &self,
        ticket_id: &str,
        webview_label: Option<&str>,
    ) -> Result<u64, FileResourceErrorV1> {
        self.validated_ticket(ticket_id, webview_label)
            .await
            .map(|ticket| ticket.size_bytes)
    }

    async fn validated_ticket(
        &self,
        ticket_id: &str,
        webview_label: Option<&str>,
    ) -> Result<FileReadTicket, FileResourceErrorV1> {
        let ticket = self
            .inner
            .read_tickets
            .lock()
            .await
            .get(ticket_id)
            .cloned()
            .ok_or_else(|| error("invalid_ticket", "file read ticket is unavailable"))?;
        if ticket.expires_at <= Instant::now() {
            self.expire_ticket_issuance(
                ticket_id,
                ticket.issuance_id,
                &ticket.renderer_lease,
                ticket.expires_at,
            )
            .await;
            return Err(error("expired_ticket", "file read ticket has expired"));
        }
        if let Some(expected_label) = ticket.webview_label.as_deref() {
            if webview_label != Some(expected_label) {
                return Err(error(
                    "unauthorized_ticket",
                    "file read ticket belongs to another renderer webview",
                ));
            }
        }
        self.ensure_ticket_lease_active(&ticket).await?;
        Ok(ticket)
    }

    async fn remove_expired_tickets(&self) {
        let now = Instant::now();
        self.inner
            .read_tickets
            .lock()
            .await
            .retain(|_, ticket| ticket.expires_at > now);
        self.inner
            .renderer_leases
            .lock()
            .await
            .retain(|_, lease| lease.expires_at > now);
    }

    async fn ensure_ticket_lease_active(
        &self,
        ticket: &FileReadTicket,
    ) -> Result<(), FileResourceErrorV1> {
        let lease_is_active = self
            .inner
            .renderer_leases
            .lock()
            .await
            .get(&ticket.renderer_lease)
            .is_some_and(|lease| {
                lease.issuance_id == ticket.issuance_id
                    && lease.subscription_id == ticket.subscription_id
                    && lease.expires_at > Instant::now()
            });
        let subscription_is_active = self
            .inner
            .subscription_resources
            .lock()
            .await
            .get(&ticket.subscription_id)
            .is_some_and(|resource_id| resource_id == &ticket.resource_id);
        if !lease_is_active || !subscription_is_active {
            return Err(error(
                "invalid_ticket",
                "file read ticket renderer lease is no longer active",
            ));
        }
        Ok(())
    }

    async fn rollback_ticket_publication(&self, ticket_id: &str, ticket: &FileReadTicket) {
        let mut tickets = self.inner.read_tickets.lock().await;
        if tickets
            .get(ticket_id)
            .is_some_and(|published| published.issuance_id == ticket.issuance_id)
        {
            tickets.remove(ticket_id);
        }
        drop(tickets);

        let mut leases = self.inner.renderer_leases.lock().await;
        if leases
            .get(&ticket.renderer_lease)
            .is_some_and(|published| published.issuance_id == ticket.issuance_id)
        {
            leases.remove(&ticket.renderer_lease);
        }
    }

    pub async fn close_all(&self) {
        let operations = self
            .inner
            .entries
            .lock()
            .await
            .values()
            .map(|entry| entry.operation.clone())
            .collect::<Vec<_>>();
        let mut operation_guards = Vec::with_capacity(operations.len());
        for operation in operations {
            operation_guards.push(operation.lock_owned().await);
        }
        self.inner.entries.lock().await.clear();
        self.inner.subscription_resources.lock().await.clear();
        self.inner.user_file_grants.lock().await.clear();
        self.inner.save_target_grants.lock().await.clear();
        self.inner.read_tickets.lock().await.clear();
        self.inner.renderer_leases.lock().await.clear();
        match self.inner.app_handle.write() {
            Ok(mut current) => *current = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
        drop(operation_guards);
    }

    #[must_use]
    pub async fn watcher_count(&self) -> usize {
        self.inner.entries.lock().await.len()
    }

    #[must_use]
    pub async fn subscriber_count(&self, resource_id: &str) -> usize {
        self.inner
            .entries
            .lock()
            .await
            .get(resource_id)
            .map(|entry| entry.subscribers.len())
            .unwrap_or_default()
    }

    #[must_use]
    pub async fn ticket_count(&self) -> usize {
        self.inner.read_tickets.lock().await.len()
    }

    #[must_use]
    pub async fn renderer_lease_count(&self) -> usize {
        self.inner.renderer_leases.lock().await.len()
    }

    #[must_use]
    pub async fn user_grant_count(&self) -> usize {
        self.inner.user_file_grants.lock().await.len()
    }

    #[cfg(test)]
    fn ticket_snapshot_bytes_in_use(&self) -> u64 {
        self.inner.ticket_snapshot_usage.load(Ordering::Acquire)
    }
}

impl Default for FileResourceRuntime {
    fn default() -> Self {
        Self::with_timing(DEFAULT_STABILITY_DELAY, DEFAULT_TICKET_TTL)
    }
}

pub(crate) fn parse_byte_range(
    range_header: Option<&str>,
    size_bytes: u64,
) -> Result<(u64, u64), FileResourceErrorV1> {
    if size_bytes == 0 {
        return Err(error(
            "range_not_satisfiable",
            "empty resource has no satisfiable byte range",
        ));
    }
    let Some(header) = range_header else {
        return Ok((0, size_bytes - 1));
    };
    let value = header
        .strip_prefix("bytes=")
        .ok_or_else(|| error("invalid_range", "range unit must be bytes"))?;
    if value.contains(',') {
        return Err(error(
            "invalid_range",
            "multiple byte ranges are not supported",
        ));
    }
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| error("invalid_range", "byte range is malformed"))?;
    if start.is_empty() {
        let suffix: u64 = end
            .parse()
            .map_err(|_| error("invalid_range", "suffix byte range is malformed"))?;
        if suffix == 0 {
            return Err(error("invalid_range", "suffix byte range is empty"));
        }
        let start = size_bytes.saturating_sub(suffix.min(size_bytes));
        return Ok((start, size_bytes - 1));
    }
    let start: u64 = start
        .parse()
        .map_err(|_| error("invalid_range", "byte range start is malformed"))?;
    if start >= size_bytes {
        return Err(error(
            "range_not_satisfiable",
            "byte range starts beyond the resource",
        ));
    }
    let end = if end.is_empty() {
        size_bytes - 1
    } else {
        end.parse()
            .map_err(|_| error("invalid_range", "byte range end is malformed"))?
    };
    if start > end {
        return Err(error("invalid_range", "byte range start exceeds its end"));
    }
    Ok((start, end.min(size_bytes - 1)))
}

async fn verified_snapshot(
    authorized: AuthorizedPath,
    limits: FileResourceLimits,
) -> Result<VerifiedFileSnapshot, FileResourceErrorV1> {
    tauri::async_runtime::spawn_blocking(move || {
        VerifiedFileSnapshot::from_authorized_path(&authorized, &limits)
    })
    .await
    .map_err(join_error)?
}

fn join_error(cause: impl std::fmt::Display) -> FileResourceErrorV1 {
    error(
        "runtime_unavailable",
        format!("file resource worker failed: {cause}"),
    )
}

fn error(code: &str, message: impl Into<String>) -> FileResourceErrorV1 {
    FileResourceErrorV1::new(code, message)
}

fn authorize_user_file_path(path: &Path) -> Result<AuthorizedPath, FileResourceErrorV1> {
    let parent = path.parent().ok_or_else(|| {
        error(
            "unavailable_path",
            "selected file does not have an authorizable parent directory",
        )
    })?;
    let parent = parent.to_str().ok_or_else(|| {
        error(
            "unavailable_path",
            "selected file parent cannot be represented losslessly as UTF-8",
        )
    })?;
    let config = AgentConfig {
        session_id: "native-picker".to_string(),
        folder: parent.to_string(),
        ..AgentConfig::default()
    };
    AuthorizedRootService::from_agent_config(&config)?.authorize_existing_file(path)
}

fn absolute_path(path: &Path) -> Result<PathBuf, FileResourceErrorV1> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|cause| {
            error(
                "unavailable_path",
                format!("cannot resolve current directory: {cause}"),
            )
        })
}

fn validate_submitted_text(
    text: &str,
    limits: &FileResourceLimits,
) -> Result<(), FileResourceErrorV1> {
    let (size_bytes, line_count) = text_size_and_line_count(text)?;
    if !limits.allows_monaco(size_bytes, line_count) {
        return Err(error(
            "file_too_large",
            "submitted text exceeds the complete model limits",
        ));
    }
    Ok(())
}

fn validate_recovery_diff_side(
    text: &str,
    limits: &FileResourceLimits,
) -> Result<(), FileResourceErrorV1> {
    let (size_bytes, line_count) = text_size_and_line_count(text)?;
    if !limits.allows_diff_side(size_bytes, line_count) {
        return Err(error(
            "file_too_large",
            "recovery text exceeds the per-side diff limits",
        ));
    }
    Ok(())
}

fn finalize_recovery_merge(
    merged: Result<String, String>,
    recovery_revision: u64,
    current_revision: u64,
    current_content_hash: String,
    disk_changed: bool,
    limits: &FileResourceLimits,
) -> Result<FileRecoveryMergeResultV1, FileResourceErrorV1> {
    let merged_text = match &merged {
        Ok(merged_text) | Err(merged_text) => merged_text,
    };
    validate_submitted_text(merged_text, limits)?;
    Ok(match merged {
        Ok(merged_text) => FileRecoveryMergeResultV1::Clean {
            recovery_revision,
            current_revision,
            current_content_hash,
            disk_changed,
            merged_text,
        },
        Err(merged_text) => FileRecoveryMergeResultV1::Conflicted {
            recovery_revision,
            current_revision,
            current_content_hash,
            disk_changed,
            merged_text,
        },
    })
}

fn text_size_and_line_count(text: &str) -> Result<(u64, u64), FileResourceErrorV1> {
    let size_bytes = u64::try_from(text.len()).map_err(|_| {
        error(
            "file_too_large",
            "submitted text cannot fit in the supported file size",
        )
    })?;
    let mut line_count = 1_u64;
    let mut previous_was_cr = false;
    for character in text.chars() {
        if previous_was_cr {
            previous_was_cr = false;
            if character == '\n' {
                continue;
            }
        }
        match character {
            '\r' => {
                line_count = line_count.saturating_add(1);
                previous_was_cr = true;
            }
            '\n' => line_count = line_count.saturating_add(1),
            _ => {}
        }
    }
    Ok((size_bytes, line_count))
}

fn prospective_save_target_canonical_path(
    grant: &SaveTargetGrant,
) -> Result<String, FileResourceErrorV1> {
    let path = match &grant.binding {
        SaveTargetBinding::Existing { snapshot, .. } => {
            return Ok(snapshot.descriptor().canonical_path.clone());
        }
        SaveTargetBinding::Missing => grant.canonical_parent.join(&grant.basename),
    };
    path.to_str().map(str::to_string).ok_or_else(|| {
        error(
            "unavailable_path",
            "selected save target cannot be represented losslessly as UTF-8",
        )
    })
}

fn verify_save_target_parent(grant: &SaveTargetGrant) -> Result<(), FileResourceErrorV1> {
    let retained_identity = FilesystemIdentity::from_file(&grant.parent).map_err(|cause| {
        error(
            "unauthorized_save_target",
            format!("selected save directory handle is unavailable: {cause}"),
        )
    })?;
    if retained_identity != grant.parent_identity {
        return Err(error(
            "unauthorized_save_target",
            "selected save directory changed identity",
        ));
    }
    let current_canonical = std::fs::canonicalize(&grant.requested_parent).map_err(|_| {
        error(
            "unauthorized_save_target",
            "selected save directory binding is unavailable",
        )
    })?;
    if current_canonical != grant.canonical_parent
        || grant.selected_path != grant.canonical_parent.join(&grant.basename)
    {
        return Err(error(
            "unauthorized_save_target",
            "selected save directory or exact basename changed binding",
        ));
    }
    let current = open_directory(&current_canonical).map_err(|_| {
        error(
            "unauthorized_save_target",
            "selected save directory cannot be reopened",
        )
    })?;
    if FilesystemIdentity::from_file(&current).map_err(|_| {
        error(
            "unauthorized_save_target",
            "selected save directory identity cannot be verified",
        )
    })? != grant.parent_identity
    {
        return Err(error(
            "unauthorized_save_target",
            "selected save directory changed identity",
        ));
    }
    Ok(())
}

fn atomic_create_text_exact(
    grant: &SaveTargetGrant,
    text: &str,
) -> Result<(), FileResourceErrorV1> {
    verify_save_target_parent(grant)?;
    match std::fs::symlink_metadata(&grant.selected_path) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(error(
                "unauthorized_save_target",
                "selected save target binding changed before use",
            ));
        }
        Err(cause) => {
            return Err(error(
                "unauthorized_save_target",
                format!("cannot verify selected save target binding: {cause}"),
            ));
        }
    }
    let staged = grant.canonical_parent.join(format!(
        ".{}.{}.wardian-save-as.tmp",
        grant.basename.to_string_lossy(),
        Uuid::new_v4().simple()
    ));
    let stage_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staged)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(cause) = stage_result {
        let _ = std::fs::remove_file(&staged);
        return Err(error(
            "unavailable_path",
            format!("cannot stage exact save target: {cause}"),
        ));
    }

    if let Err(failure) = verify_save_target_parent(grant) {
        let _ = std::fs::remove_file(&staged);
        return Err(failure);
    }
    if let Err(cause) = commit_staged_new_exact(&staged, &grant.selected_path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error(
            "unauthorized_save_target",
            format!("selected save target binding changed before commit: {cause}"),
        ));
    }
    #[cfg(not(windows))]
    grant.parent.sync_all().map_err(|cause| {
        error(
            "unavailable_path",
            format!("cannot flush selected save directory: {cause}"),
        )
    })?;
    Ok(())
}

#[cfg(not(windows))]
fn commit_staged_new_exact(staged: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::hard_link(staged, target)?;
    std::fs::remove_file(staged)
}

#[cfg(windows)]
fn commit_staged_new_exact(staged: &Path, target: &Path) -> std::io::Result<()> {
    let staged = wide_null(staged.as_os_str());
    let target = wide_null(target.as_os_str());
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    let moved = unsafe { MoveFileExW(staged.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
}

#[cfg(unix)]
impl FilesystemIdentity {
    fn from_file(file: &File) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = file.metadata()?;
        Ok(Self {
            volume: metadata.dev(),
            file: metadata.ino(),
        })
    }
}

#[cfg(windows)]
impl FilesystemIdentity {
    fn from_file(file: &File) -> std::io::Result<Self> {
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
        let succeeded =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if succeeded == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let information = unsafe { information.assume_init() };
        Ok(Self {
            volume: u64::from(information.dwVolumeSerialNumber),
            file: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        })
    }
}

#[cfg(not(any(unix, windows)))]
impl FilesystemIdentity {
    fn from_file(file: &File) -> std::io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            volume: 0,
            file: metadata.len(),
        })
    }
}

#[cfg(not(windows))]
fn open_directory(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
fn open_directory(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn same_subscriptions(entry: &FileResourceEntry, expected: &[String]) -> bool {
    entry.subscribers.len() == expected.len()
        && expected
            .iter()
            .all(|subscription_id| entry.subscribers.contains_key(subscription_id))
}

fn file_resource_id(canonical_path: &str) -> String {
    #[cfg(windows)]
    let canonical_path = canonical_path.replace('\\', "/");
    format!("file:{canonical_path}")
}

fn default_recovery_root() -> Option<PathBuf> {
    crate::utils::fs::get_wardian_home().map(|home| home.join("files").join("recovery"))
}

fn recovery_record_ids(recovery_root: &Path) -> Result<Vec<String>, FileResourceErrorV1> {
    match std::fs::symlink_metadata(recovery_root) {
        Ok(_) => validate_recovery_root(recovery_root)?,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(cause) => {
            return Err(error(
                "recovery_unavailable",
                format!("cannot inspect recovery root: {cause}"),
            ));
        }
    }
    let entries = std::fs::read_dir(recovery_root).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot enumerate recovery records: {cause}"),
        )
    })?;
    let mut recovery_ids = Vec::new();
    for entry in entries.flatten() {
        let Some(recovery_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(parsed) = Uuid::parse_str(&recovery_id) else {
            continue;
        };
        if parsed.to_string() != recovery_id {
            continue;
        }
        let Ok(metadata) = entry.path().symlink_metadata() else {
            continue;
        };
        if metadata.file_type().is_dir()
            && validate_direct_child_directory(recovery_root, &entry.path(), "record directory")
                .is_ok()
        {
            recovery_ids.push(recovery_id);
        }
    }
    recovery_ids.sort();
    Ok(recovery_ids)
}

fn recovery_record_dir(
    recovery_root: &Path,
    recovery_id: &str,
) -> Result<PathBuf, FileResourceErrorV1> {
    let parsed = Uuid::parse_str(recovery_id).map_err(|_| {
        error(
            "invalid_request",
            "recovery id is not a valid opaque identifier",
        )
    })?;
    if parsed.to_string() != recovery_id {
        return Err(error(
            "invalid_request",
            "recovery id is not in canonical form",
        ));
    }
    Ok(recovery_root.join(recovery_id))
}

fn validate_direct_child_directory(
    parent: &Path,
    child: &Path,
    label: &str,
) -> Result<(), FileResourceErrorV1> {
    let parent_metadata = std::fs::symlink_metadata(parent).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot inspect recovery {label} parent: {cause}"),
        )
    })?;
    let child_metadata = std::fs::symlink_metadata(child).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot inspect recovery {label}: {cause}"),
        )
    })?;
    if !parent_metadata.file_type().is_dir() || !child_metadata.file_type().is_dir() {
        return Err(error(
            "invalid_recovery",
            format!("recovery {label} is not an ordinary directory"),
        ));
    }
    let canonical_parent = std::fs::canonicalize(parent).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot resolve recovery {label} parent: {cause}"),
        )
    })?;
    let canonical_child = std::fs::canonicalize(child).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot resolve recovery {label}: {cause}"),
        )
    })?;
    if canonical_child.parent() != Some(canonical_parent.as_path()) {
        return Err(error(
            "invalid_recovery",
            format!("recovery {label} is not a direct child of its backend-owned parent"),
        ));
    }
    Ok(())
}

fn validate_recovery_root(recovery_root: &Path) -> Result<(), FileResourceErrorV1> {
    let metadata = std::fs::symlink_metadata(recovery_root).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot inspect recovery root: {cause}"),
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(error(
            "invalid_recovery",
            "recovery root is not an ordinary directory",
        ));
    }
    Ok(())
}

fn validate_recovery_record_dir(
    recovery_root: &Path,
    recovery_id: &str,
) -> Result<PathBuf, FileResourceErrorV1> {
    let record_dir = recovery_record_dir(recovery_root, recovery_id)?;
    match std::fs::symlink_metadata(&record_dir) {
        Ok(_) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return Err(error(
                "recovery_not_found",
                "recovery checkpoint does not exist",
            ));
        }
        Err(cause) => {
            return Err(error(
                "recovery_unavailable",
                format!("cannot inspect recovery checkpoint: {cause}"),
            ));
        }
    }
    validate_direct_child_directory(recovery_root, &record_dir, "record directory")?;
    Ok(record_dir)
}

fn recovery_checkpoint_metadata(
    manifest: &FileRecoveryManifestV1,
    file_authorization_error: Option<FileResourceErrorV1>,
) -> FileRecoveryCheckpointV1 {
    FileRecoveryCheckpointV1 {
        schema: 1,
        recovery_id: manifest.recovery_id.clone(),
        resource_key: manifest.resource_key.clone(),
        base_content_hash: manifest.base_content_hash.clone(),
        base_opaque_revision: manifest.base_opaque_revision.clone(),
        recovery_revision: manifest.recovery_revision,
        created_at_ms: manifest.created_at_ms,
        updated_at_ms: manifest.updated_at_ms,
        file_authorization_error,
    }
}

fn recovery_summary_metadata(manifest: &FileRecoveryManifestV1) -> FileRecoverySummaryV1 {
    FileRecoverySummaryV1 {
        schema: 1,
        recovery_id: manifest.recovery_id.clone(),
        resource_key: manifest.resource_key.clone(),
        display_name: manifest.display_name.clone(),
        extension: manifest.extension.clone(),
        mime_type: manifest.mime_type.clone(),
        base_content_hash: manifest.base_content_hash.clone(),
        base_opaque_revision: manifest.base_opaque_revision.clone(),
        recovery_revision: manifest.recovery_revision,
        created_at_ms: manifest.created_at_ms,
        updated_at_ms: manifest.updated_at_ms,
    }
}

fn authorize_recovery_manifest(
    manifest: &FileRecoveryManifestV1,
    resource_key: &str,
    webview_scope: &str,
) -> Result<(), FileResourceErrorV1> {
    if manifest.schema != 1
        || manifest.resource_key != resource_key
        || manifest.webview_scope != webview_scope
    {
        return Err(error(
            "unauthorized_recovery",
            "recovery does not belong to this resource and webview scope",
        ));
    }
    Ok(())
}

fn load_recovery_manifest(
    recovery_root: &Path,
    recovery_id: &str,
) -> Result<FileRecoveryManifestV1, FileResourceErrorV1> {
    let record_dir = validate_recovery_record_dir(recovery_root, recovery_id)?;
    let path = record_dir.join("manifest.json");
    let metadata = std::fs::symlink_metadata(&path).map_err(|cause| {
        if cause.kind() == std::io::ErrorKind::NotFound {
            error("recovery_not_found", "recovery checkpoint does not exist")
        } else {
            error(
                "recovery_unavailable",
                format!("cannot inspect recovery manifest: {cause}"),
            )
        }
    })?;
    if !metadata.file_type().is_file() || metadata.len() > 64 * 1024 {
        return Err(error(
            "invalid_recovery",
            "recovery manifest is not a bounded ordinary file",
        ));
    }
    let bytes = std::fs::read(&path).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot read recovery manifest: {cause}"),
        )
    })?;
    let manifest: FileRecoveryManifestV1 = serde_json::from_slice(&bytes).map_err(|cause| {
        error(
            "invalid_recovery",
            format!("recovery manifest is invalid: {cause}"),
        )
    })?;
    if manifest.recovery_id != recovery_id || manifest.schema != 1 {
        return Err(error(
            "invalid_recovery",
            "recovery manifest identity or schema is invalid",
        ));
    }
    Ok(manifest)
}

fn recovery_blob_name(text: &str) -> String {
    format!("sha256-{:x}.txt", Sha256::digest(text.as_bytes()))
}

fn is_recovery_blob_name(blob_name: &str) -> bool {
    blob_name.starts_with("sha256-")
        && blob_name.ends_with(".txt")
        && blob_name.len() == "sha256-".len() + 64 + ".txt".len()
        && blob_name["sha256-".len()..blob_name.len() - ".txt".len()]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn write_recovery_blob(record_dir: &Path, text: &str) -> Result<String, FileResourceErrorV1> {
    let blob_name = recovery_blob_name(text);
    let blobs_dir = record_dir.join("blobs");
    std::fs::create_dir_all(&blobs_dir).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot create recovery blob directory: {cause}"),
        )
    })?;
    validate_direct_child_directory(record_dir, &blobs_dir, "blob directory")?;
    let path = blobs_dir.join(&blob_name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            validate_existing_recovery_blob(&path, &metadata, text)?;
            return Ok(blob_name);
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => {
            return Err(error(
                "invalid_recovery",
                format!("cannot inspect recovery blob target: {cause}"),
            ));
        }
    }
    match OpenOptions::new().create_new(true).write(true).open(&path) {
        Ok(mut file) => {
            file.write_all(text.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|cause| {
                    let _ = std::fs::remove_file(&path);
                    error(
                        "recovery_unavailable",
                        format!("cannot write recovery blob: {cause}"),
                    )
                })?;
            sync_recovery_directory(&blobs_dir).map_err(|cause| {
                error(
                    "recovery_unavailable",
                    format!("cannot flush recovery blob directory: {cause}"),
                )
            })?;
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&path).map_err(|metadata_cause| {
                error(
                    "invalid_recovery",
                    format!("cannot inspect existing recovery blob: {metadata_cause}"),
                )
            })?;
            validate_existing_recovery_blob(&path, &metadata, text)?;
        }
        Err(cause) => {
            return Err(error(
                "recovery_unavailable",
                format!("cannot create recovery blob: {cause}"),
            ));
        }
    }
    Ok(blob_name)
}

fn validate_existing_recovery_blob(
    path: &Path,
    metadata: &std::fs::Metadata,
    text: &str,
) -> Result<(), FileResourceErrorV1> {
    let expected_length = u64::try_from(text.len()).map_err(|_| {
        error(
            "file_too_large",
            "recovery blob length cannot fit in the supported size",
        )
    })?;
    if !metadata.file_type().is_file() || metadata.len() != expected_length {
        return Err(error(
            "invalid_recovery",
            "existing recovery blob is not the expected bounded ordinary file",
        ));
    }
    let existing = std::fs::read(path).map_err(|cause| {
        error(
            "invalid_recovery",
            format!("cannot verify existing recovery blob: {cause}"),
        )
    })?;
    if existing != text.as_bytes() {
        return Err(error(
            "invalid_recovery",
            "hash-addressed recovery blob contains different bytes",
        ));
    }
    Ok(())
}

fn read_recovery_blob(
    recovery_root: &Path,
    manifest: &FileRecoveryManifestV1,
    blob_name: &str,
    limits: &FileResourceLimits,
) -> Result<String, FileResourceErrorV1> {
    let path = validate_recovery_blob_metadata(recovery_root, manifest, blob_name, limits)?;
    let bytes = std::fs::read(&path).map_err(|cause| {
        error(
            "invalid_recovery",
            format!("cannot read recovery blob: {cause}"),
        )
    })?;
    let text = String::from_utf8(bytes)
        .map_err(|_| error("invalid_recovery", "recovery blob is not valid UTF-8 text"))?;
    if recovery_blob_name(&text) != blob_name {
        return Err(error(
            "invalid_recovery",
            "recovery blob hash does not match its immutable name",
        ));
    }
    validate_submitted_text(&text, limits).map_err(|_| {
        error(
            "invalid_recovery",
            "recovery blob exceeds the complete text-model limits",
        )
    })?;
    Ok(text)
}

fn validate_recovery_blob_metadata(
    recovery_root: &Path,
    manifest: &FileRecoveryManifestV1,
    blob_name: &str,
    limits: &FileResourceLimits,
) -> Result<PathBuf, FileResourceErrorV1> {
    if !is_recovery_blob_name(blob_name) {
        return Err(error(
            "invalid_recovery",
            "recovery blob name is not hash-addressed",
        ));
    }
    let record_dir = validate_recovery_record_dir(recovery_root, &manifest.recovery_id)?;
    let blobs_dir = record_dir.join("blobs");
    validate_direct_child_directory(&record_dir, &blobs_dir, "blob directory")?;
    let path = blobs_dir.join(blob_name);
    let metadata = std::fs::symlink_metadata(&path).map_err(|cause| {
        error(
            "invalid_recovery",
            format!("recovery blob is unavailable: {cause}"),
        )
    })?;
    if !metadata.file_type().is_file() || metadata.len() > limits.monaco_max_size_bytes {
        return Err(error(
            "invalid_recovery",
            "recovery blob is not a bounded ordinary file",
        ));
    }
    Ok(path)
}

fn sweep_recovery_store(
    recovery_root: &Path,
    orphan_grace_period: Duration,
) -> Result<(), FileResourceErrorV1> {
    for recovery_id in recovery_record_ids(recovery_root)? {
        match load_recovery_manifest(recovery_root, &recovery_id) {
            Ok(manifest) => {
                garbage_collect_recovery_blobs(recovery_root, &manifest, orphan_grace_period)
            }
            Err(failure) if failure.code() == "recovery_not_found" => {
                let record_dir = validate_recovery_record_dir(recovery_root, &recovery_id)?;
                let metadata = std::fs::symlink_metadata(&record_dir).map_err(|cause| {
                    error(
                        "recovery_unavailable",
                        format!("cannot inspect incomplete recovery record: {cause}"),
                    )
                })?;
                let old_enough = metadata
                    .modified()
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age >= orphan_grace_period);
                if old_enough {
                    remove_recovery_record(recovery_root, &recovery_id)?;
                }
            }
            Err(_) => {
                // Malformed or temporarily unreadable records remain intact.
                // Admission accounting below still charges their directories
                // and ordinary body files against the bounded store.
            }
        }
    }
    Ok(())
}

fn recovery_store_usage(
    recovery_root: &Path,
) -> Result<FileRecoveryStoreUsage, FileResourceErrorV1> {
    let recovery_ids = recovery_record_ids(recovery_root)?;
    let mut usage = FileRecoveryStoreUsage {
        records: recovery_ids.len(),
        body_bytes: 0,
    };
    for recovery_id in recovery_ids {
        let record_dir = recovery_record_dir(recovery_root, &recovery_id)?;
        let blobs_dir = record_dir.join("blobs");
        let metadata = match std::fs::symlink_metadata(&blobs_dir) {
            Ok(metadata) => metadata,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => {
                return Err(error(
                    "recovery_unavailable",
                    format!("cannot inspect recovery blob directory: {cause}"),
                ));
            }
        };
        if !metadata.file_type().is_dir()
            || validate_direct_child_directory(&record_dir, &blobs_dir, "blob directory").is_err()
        {
            continue;
        }
        let entries = std::fs::read_dir(&blobs_dir).map_err(|cause| {
            error(
                "recovery_unavailable",
                format!("cannot enumerate recovery blobs: {cause}"),
            )
        })?;
        for entry in entries.flatten() {
            let Ok(metadata) = entry.path().symlink_metadata() else {
                continue;
            };
            if metadata.file_type().is_file() {
                usage.body_bytes = usage.body_bytes.saturating_add(metadata.len());
            }
        }
    }
    Ok(usage)
}

fn enforce_recovery_admission(
    recovery_root: &Path,
    recovery_id: &str,
    base: &str,
    buffer: &str,
    limits: FileRecoveryStoreLimits,
) -> Result<(), FileResourceErrorV1> {
    let usage = recovery_store_usage(recovery_root)?;
    let record_dir = recovery_record_dir(recovery_root, recovery_id)?;
    let record_exists = match std::fs::symlink_metadata(&record_dir) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err(error(
                    "invalid_recovery",
                    "recovery record target is not an ordinary directory",
                ));
            }
            true
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => false,
        Err(cause) => {
            return Err(error(
                "recovery_unavailable",
                format!("cannot inspect recovery record admission target: {cause}"),
            ));
        }
    };
    if !record_exists && usage.records >= limits.max_records {
        return Err(error(
            "recovery_capacity_exceeded",
            "durable editor recovery record limit is reached",
        ));
    }

    let mut additional_bytes = 0_u64;
    let mut prospective_names = HashSet::new();
    for text in [base, buffer] {
        let blob_name = recovery_blob_name(text);
        if !prospective_names.insert(blob_name.clone()) {
            continue;
        }
        let already_exists = if record_exists {
            match std::fs::symlink_metadata(record_dir.join("blobs").join(blob_name)) {
                Ok(_) => true,
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => false,
                Err(cause) => {
                    return Err(error(
                        "recovery_unavailable",
                        format!("cannot inspect recovery blob admission target: {cause}"),
                    ));
                }
            }
        } else {
            false
        };
        if !already_exists {
            let length = u64::try_from(text.len()).map_err(|_| {
                error(
                    "recovery_capacity_exceeded",
                    "recovery body length exceeds the storage budget representation",
                )
            })?;
            additional_bytes = additional_bytes.saturating_add(length);
        }
    }
    if usage.body_bytes.saturating_add(additional_bytes) > limits.max_body_bytes {
        return Err(error(
            "recovery_capacity_exceeded",
            "durable editor recovery body-byte limit is reached",
        ));
    }
    Ok(())
}

fn write_recovery_checkpoint(
    recovery_root: &Path,
    mut manifest: FileRecoveryManifestV1,
    base: &str,
    buffer: &str,
    now: u64,
    fail_before_manifest: bool,
    orphan_grace_period: Duration,
) -> Result<FileRecoveryManifestV1, FileResourceErrorV1> {
    let record_dir = recovery_record_dir(recovery_root, &manifest.recovery_id)?;
    std::fs::create_dir_all(recovery_root).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot create recovery root: {cause}"),
        )
    })?;
    validate_recovery_root(recovery_root)?;
    std::fs::create_dir(&record_dir)
        .or_else(|cause| {
            if cause.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(cause)
            }
        })
        .map_err(|cause| {
            error(
                "recovery_unavailable",
                format!("cannot create recovery directory: {cause}"),
            )
        })?;
    validate_recovery_record_dir(recovery_root, &manifest.recovery_id)?;
    manifest.base_blob = write_recovery_blob(&record_dir, base)?;
    manifest.buffer_blob = write_recovery_blob(&record_dir, buffer)?;
    manifest.recovery_revision = manifest
        .recovery_revision
        .checked_add(1)
        .ok_or_else(|| error("recovery_conflict", "recovery revision is exhausted"))?;
    manifest.updated_at_ms = now;
    if fail_before_manifest {
        return Err(error(
            "recovery_unavailable",
            "injected failure before recovery manifest replacement",
        ));
    }
    wardian_core::conversations::write_json_atomic(&record_dir.join("manifest.json"), &manifest)
        .map_err(|cause| {
            error(
                "recovery_unavailable",
                format!("cannot commit recovery manifest: {cause}"),
            )
        })?;
    garbage_collect_recovery_blobs(recovery_root, &manifest, orphan_grace_period);
    Ok(manifest)
}

#[cfg(not(windows))]
fn sync_recovery_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_recovery_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn garbage_collect_recovery_blobs(
    recovery_root: &Path,
    manifest: &FileRecoveryManifestV1,
    orphan_grace_period: Duration,
) {
    let Ok(record_dir) = validate_recovery_record_dir(recovery_root, &manifest.recovery_id) else {
        return;
    };
    let blobs_dir = record_dir.join("blobs");
    if validate_direct_child_directory(&record_dir, &blobs_dir, "blob directory").is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(blobs_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_recovery_blob_name(name)
            || name == manifest.base_blob
            || name == manifest.buffer_blob
        {
            continue;
        }
        let Ok(metadata) = entry.path().symlink_metadata() else {
            continue;
        };
        if !metadata.file_type().is_file()
            || metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_none_or(|age| age < orphan_grace_period)
        {
            continue;
        }
        let _ = std::fs::remove_file(entry.path());
    }
}

fn remove_recovery_record(
    recovery_root: &Path,
    recovery_id: &str,
) -> Result<(), FileResourceErrorV1> {
    let record_dir = validate_recovery_record_dir(recovery_root, recovery_id)?;
    let root = std::fs::canonicalize(recovery_root).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot resolve recovery root before discard: {cause}"),
        )
    })?;
    let record = std::fs::canonicalize(&record_dir).map_err(|cause| {
        if cause.kind() == std::io::ErrorKind::NotFound {
            error("recovery_not_found", "recovery checkpoint does not exist")
        } else {
            error(
                "recovery_unavailable",
                format!("cannot resolve recovery checkpoint before discard: {cause}"),
            )
        }
    })?;
    let metadata = std::fs::symlink_metadata(&record).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot inspect recovery checkpoint before discard: {cause}"),
        )
    })?;
    if !metadata.file_type().is_dir() || record.parent() != Some(root.as_path()) {
        return Err(error(
            "invalid_recovery",
            "recovery checkpoint is outside the configured recovery root",
        ));
    }
    std::fs::remove_dir_all(&record).map_err(|cause| {
        error(
            "recovery_unavailable",
            format!("cannot discard recovery checkpoint: {cause}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};
    use wardian_core::models::AgentConfig;

    fn agent_config(agent_id: &str, root: &Path) -> AgentConfig {
        AgentConfig {
            session_id: agent_id.to_string(),
            folder: root.to_string_lossy().into_owned(),
            ..AgentConfig::default()
        }
    }

    fn test_runtime() -> FileResourceRuntime {
        FileResourceRuntime::with_timing(Duration::from_millis(150), Duration::from_secs(60))
    }

    #[tokio::test]
    async fn file_recovery_checkpoint_create_update_enforces_cas_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base text").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let created = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "main",
                "first edit",
            )
            .await
            .expect("create recovery");
        assert_eq!(created.recovery_revision, 1);

        let wrong_scope = runtime
            .checkpoint_recovery(
                Some(&created.recovery_id),
                Some(created.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "other",
                "cross-scope edit",
            )
            .await
            .expect_err("another webview scope must not update recovery");
        assert_eq!(wrong_scope.code(), "unauthorized_recovery");

        let updated = runtime
            .checkpoint_recovery(
                Some(&created.recovery_id),
                Some(created.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "main",
                "second edit",
            )
            .await
            .expect("update recovery");
        assert_eq!(updated.recovery_revision, 2);

        let conflict = runtime
            .checkpoint_recovery(
                Some(&created.recovery_id),
                Some(created.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "main",
                "stale edit",
            )
            .await
            .expect_err("stale recovery CAS must fail");
        assert_eq!(conflict.code(), "recovery_conflict");
    }

    #[tokio::test]
    async fn file_recovery_cas_update_can_advance_base_after_guarded_save() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let first = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "saved base",
            )
            .await
            .expect("first recovery");

        let (saved_revision, saved_hash) = match runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "saved base",
            )
            .await
            .expect("guarded save")
        {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved result, got {other:?}"),
        };
        assert!(saved_revision > opened.revision);
        let updated = runtime
            .checkpoint_recovery(
                Some(&first.recovery_id),
                Some(first.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &saved_hash,
                "saved base",
                &opened.resource_id,
                "main",
                "saved base\nnext edit",
            )
            .await
            .expect("next edit advances the existing recovery base");
        assert_eq!(updated.recovery_revision, first.recovery_revision + 1);
        assert_eq!(updated.base_content_hash, saved_hash);
        assert_ne!(updated.base_opaque_revision, first.base_opaque_revision);

        drop(runtime);
        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let restored = restarted
            .get_recovery(&updated.recovery_id, &opened.resource_id, "main")
            .await
            .expect("restart reads one complete advanced generation");
        assert_eq!(restored.base_content_hash, saved_hash);
        assert_eq!(restored.base, "saved base");
        assert_eq!(restored.buffer, "saved base\nnext edit");
        assert_eq!(fs::read_to_string(path).expect("disk bytes"), "saved base");
    }

    #[tokio::test]
    async fn file_recovery_checkpoint_rejects_unverified_or_oversized_submitted_bases() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let mismatch = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "forged base",
                &opened.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("mismatched submitted base and hash must fail closed");
        assert_eq!(mismatch.code(), "invalid_request");

        let other_path = workspace.join("other.txt");
        fs::write(&other_path, "other base").expect("other fixture");
        let other = runtime
            .open_agent_file("agent-a", &config, &other_path, None)
            .await
            .expect("open other resource");
        let wrong_subscription = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &other.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("another resource subscription must not checkpoint this resource");
        assert_eq!(wrong_subscription.code(), "unauthorized_resource");
        let wrong_resource_key = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &other.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("another resource key must not receive this recovery");
        assert_eq!(wrong_resource_key.code(), "unauthorized_resource");

        let oversized = "x".repeat(
            usize::try_from(FileResourceLimits::default().monaco_max_size_bytes)
                .expect("limit fits usize")
                + 1,
        );
        let oversized_hash = format!("sha256:{:x}", Sha256::digest(oversized.as_bytes()));
        let too_large = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &oversized_hash,
                &oversized,
                &opened.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("oversized submitted base must fail closed");
        assert_eq!(too_large.code(), "file_too_large");
        assert!(runtime
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("failed requests leave no recovery")
            .is_empty());
    }

    #[tokio::test]
    async fn file_recovery_first_checkpoint_survives_an_advanced_disk_head() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        let original_base = "one\ntwo\nthree\n";
        let dirty_buffer = "ONE\ntwo\nthree\n";
        fs::write(&path, original_base).expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        fs::write(&path, "one\ntwo\nTHREE\n").expect("external disk update");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                original_base,
                &opened.resource_id,
                "main",
                dirty_buffer,
            )
            .await
            .expect("hash-verified editor base must remain checkpointable");

        drop(runtime);
        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let discovered = restarted
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("list after restart");
        assert_eq!(discovered.len(), 1);
        let restored = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect("get after restart");
        assert_eq!(restored.base, original_base);
        assert_eq!(restored.buffer, dirty_buffer);

        let reopened = restarted
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("restore live authorization");
        let merged = restarted
            .merge_recovery(
                &checkpoint.recovery_id,
                checkpoint.recovery_revision,
                &opened.resource_id,
                "main",
                &reopened.resource_id,
                &reopened.subscription_id,
            )
            .await
            .expect("merge against advanced disk head");
        match merged {
            FileRecoveryMergeResultV1::Clean {
                disk_changed,
                merged_text,
                ..
            } => {
                assert!(disk_changed);
                assert_eq!(merged_text, "ONE\ntwo\nTHREE\n");
            }
            other => panic!("expected clean stale merge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn file_recovery_restart_read_is_scoped_and_discard_is_cas_guarded() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "stored base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "stored base",
                &opened.resource_id,
                "main",
                "stored buffer",
            )
            .await
            .expect("checkpoint");
        drop(runtime);
        fs::write(&path, "current disk secret").expect("external write");

        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let restored = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect("read-only restart restore");
        assert_eq!(restored.base, "stored base");
        assert_eq!(restored.buffer, "stored buffer");
        assert!(!restored.base.contains("current disk secret"));
        assert!(!restored.buffer.contains("current disk secret"));
        let current_read = restarted
            .read_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                Some(&config),
            )
            .await
            .expect_err("recovery-only runtime must not revive a file subscription");
        assert_eq!(current_read.code(), "resource_not_found");

        let wrong_scope = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "other")
            .await
            .expect_err("another webview must not read recovery");
        assert_eq!(wrong_scope.code(), "unauthorized_recovery");
        let wrong_resource = restarted
            .get_recovery(&checkpoint.recovery_id, "file:/another.txt", "main")
            .await
            .expect_err("another resource must not read recovery");
        assert_eq!(wrong_resource.code(), "unauthorized_recovery");

        let stale_discard = restarted
            .discard_recovery(
                &checkpoint.recovery_id,
                checkpoint.recovery_revision + 1,
                &opened.resource_id,
                "main",
            )
            .await
            .expect_err("discard must enforce recovery CAS");
        assert_eq!(stale_discard.code(), "recovery_conflict");
        restarted
            .discard_recovery(
                &checkpoint.recovery_id,
                checkpoint.recovery_revision,
                &opened.resource_id,
                "main",
            )
            .await
            .expect("discard");
        let discarded = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("discarded recovery must stay gone");
        assert_eq!(discarded.code(), "recovery_not_found");
        assert_eq!(
            fs::read_to_string(path).expect("disk bytes"),
            "current disk secret"
        );
    }

    #[tokio::test]
    async fn file_recovery_restart_discovers_and_recheckpoints_without_private_base_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let saved = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "revision two",
            )
            .await
            .expect("advance logical revision");
        let (base_revision, base_hash) = match saved {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved revision, got {other:?}"),
        };
        assert!(base_revision > 1);
        runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &base_hash,
                "revision two",
                &opened.resource_id,
                "main",
                "first recovered edit",
            )
            .await
            .expect("checkpoint after later logical revision");
        let resource_key = opened.resource_id.clone();
        drop(runtime);

        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let discovered = restarted
            .list_recoveries(&resource_key, "main")
            .await
            .expect("discover after process restart");
        assert_eq!(discovered.len(), 1);
        assert!(restarted
            .list_recoveries(&resource_key, "other")
            .await
            .expect("wrong scope discovery")
            .is_empty());
        assert!(restarted
            .list_recoveries("file:/another.txt", "main")
            .await
            .expect("wrong resource discovery")
            .is_empty());
        let restored = restarted
            .get_recovery(&discovered[0].recovery_id, &resource_key, "main")
            .await
            .expect("restore discovered recovery");
        assert_eq!(restored.buffer, "first recovered edit");

        let reopened = restarted
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("new live authorization");
        assert_eq!(reopened.revision, 1, "runtime revision is process-local");
        let updated = restarted
            .checkpoint_recovery(
                Some(&discovered[0].recovery_id),
                Some(discovered[0].recovery_revision),
                &reopened.resource_id,
                &reopened.subscription_id,
                &base_hash,
                "revision two",
                &resource_key,
                "main",
                "second recovered edit",
            )
            .await
            .expect("checkpoint recovered buffer with new runtime revision");
        assert_eq!(
            updated.recovery_revision,
            discovered[0].recovery_revision + 1
        );
        assert_eq!(
            restarted
                .get_recovery(&updated.recovery_id, &resource_key, "main")
                .await
                .expect("updated recovery")
                .buffer,
            "second recovered edit"
        );
    }

    #[tokio::test]
    async fn file_recovery_manifest_last_failure_never_mixes_blob_generations() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "durable base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let first = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "durable base",
                &opened.resource_id,
                "main",
                "first generation",
            )
            .await
            .expect("first checkpoint");
        let first_manifest =
            load_recovery_manifest(&recovery_root, &first.recovery_id).expect("first manifest");
        assert!(first_manifest.base_blob.starts_with("sha256-"));
        assert!(first_manifest.buffer_blob.starts_with("sha256-"));

        let (rebased_revision, rebased_hash) = match runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "rebased base",
            )
            .await
            .expect("advance editor base")
        {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved result, got {other:?}"),
        };
        assert!(rebased_revision > opened.revision);
        runtime.fail_next_recovery_before_manifest();
        let interrupted = runtime
            .checkpoint_recovery(
                Some(&first.recovery_id),
                Some(first.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &rebased_hash,
                "rebased base",
                &opened.resource_id,
                "main",
                "uncommitted generation",
            )
            .await
            .expect_err("fault before manifest must fail checkpoint");
        assert_eq!(interrupted.code(), "recovery_unavailable");
        drop(runtime);

        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let restored = restarted
            .get_recovery(&first.recovery_id, &opened.resource_id, "main")
            .await
            .expect("restore committed generation");
        assert_eq!(restored.recovery_revision, first.recovery_revision);
        assert_eq!(restored.base_content_hash, first.base_content_hash);
        assert_eq!(restored.base_opaque_revision, first.base_opaque_revision);
        assert_eq!(restored.base, "durable base");
        assert_eq!(restored.buffer, "first generation");
        assert_ne!(restored.buffer, "uncommitted generation");
        let manifest = load_recovery_manifest(
            &restarted.recovery_root().expect("recovery root"),
            &first.recovery_id,
        )
        .expect("committed manifest");
        let blob_count = fs::read_dir(
            restarted
                .recovery_root()
                .expect("recovery root")
                .join(&first.recovery_id)
                .join("blobs"),
        )
        .expect("blob directory")
        .count();
        assert!(
            blob_count > 2,
            "fresh unreachable blob is retained conservatively"
        );
        assert!(is_recovery_blob_name(&manifest.base_blob));
    }

    #[tokio::test]
    async fn file_recovery_store_sweeps_crash_debris_and_enforces_admission_budgets() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        runtime.configure_recovery_store_for_test(2, 18, Duration::ZERO);
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        runtime.fail_next_recovery_before_manifest();
        let interrupted = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "lost",
            )
            .await
            .expect_err("initial manifest-last fault must fail");
        assert_eq!(interrupted.code(), "recovery_unavailable");
        assert_eq!(
            fs::read_dir(&recovery_root).expect("recovery root").count(),
            1
        );
        assert!(runtime
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("sweep manifestless record")
            .is_empty());
        assert_eq!(
            fs::read_dir(&recovery_root).expect("recovery root").count(),
            0
        );

        runtime.configure_recovery_store_for_test(2, 18, RECOVERY_ORPHAN_GRACE_PERIOD);
        let first = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "one",
            )
            .await
            .expect("first bounded record");
        let second = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "two",
            )
            .await
            .expect("second bounded record");
        let record_limit = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "new",
            )
            .await
            .expect_err("third recovery id must exceed record budget");
        assert_eq!(record_limit.code(), "recovery_capacity_exceeded");

        let byte_limit = runtime
            .checkpoint_recovery(
                Some(&first.recovery_id),
                Some(first.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "three",
            )
            .await
            .expect_err("fresh immutable generations must count toward byte budget");
        assert_eq!(byte_limit.code(), "recovery_capacity_exceeded");

        let second_manifest =
            load_recovery_manifest(&recovery_root, &second.recovery_id).expect("second manifest");
        let second_record = recovery_root.join(&second.recovery_id);
        let orphan_blob = write_recovery_blob(&second_record, "orphan").expect("orphan fixture");
        runtime.configure_recovery_store_for_test(2, 18, Duration::ZERO);
        let discovered = runtime
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("store-wide sweep");
        assert_eq!(discovered.len(), 2);
        assert!(!second_record.join("blobs").join(orphan_blob).exists());
        assert!(second_record
            .join("blobs")
            .join(second_manifest.buffer_blob)
            .is_file());
        assert_eq!(
            runtime
                .get_recovery(&first.recovery_id, &opened.resource_id, "main")
                .await
                .expect("live recovery retained")
                .buffer,
            "one"
        );
    }

    #[test]
    fn file_recovery_merge_rejects_an_oversized_final_model() {
        let limits = FileResourceLimits {
            monaco_max_size_bytes: 8,
            ..FileResourceLimits::default()
        };
        let result = finalize_recovery_merge(
            Err("conflict!".to_string()),
            2,
            4,
            "sha256:current".to_string(),
            true,
            &limits,
        )
        .expect_err("final conflict-marker model must be bounded");
        assert_eq!(result.code(), "file_too_large");
    }

    #[tokio::test]
    async fn file_recovery_merge_reports_clean_and_conflicted_stale_outcomes() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let clean_checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "one\ntwo\nthree\n",
                &opened.resource_id,
                "main",
                "ONE\ntwo\nthree\n",
            )
            .await
            .expect("clean checkpoint");
        fs::write(&path, "one\ntwo\nTHREE\n").expect("external clean edit");

        let clean = runtime
            .merge_recovery(
                &clean_checkpoint.recovery_id,
                clean_checkpoint.recovery_revision,
                &opened.resource_id,
                "main",
                &opened.resource_id,
                &opened.subscription_id,
            )
            .await
            .expect("clean merge");
        match clean {
            FileRecoveryMergeResultV1::Clean {
                disk_changed,
                merged_text,
                ..
            } => {
                assert!(disk_changed);
                assert_eq!(merged_text, "ONE\ntwo\nTHREE\n");
            }
            other => panic!("expected clean merge, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(&path).expect("disk after clean merge"),
            "one\ntwo\nTHREE\n"
        );

        let conflict_path = workspace.join("conflict.txt");
        fs::write(&conflict_path, "shared line\n").expect("conflict fixture");
        let conflict_opened = runtime
            .open_agent_file("agent-a", &config, &conflict_path, None)
            .await
            .expect("open conflict");
        let conflict_checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &conflict_opened.resource_id,
                &conflict_opened.subscription_id,
                &conflict_opened.descriptor.content_hash,
                "shared line\n",
                &conflict_opened.resource_id,
                "main",
                "buffer line\n",
            )
            .await
            .expect("conflict checkpoint");
        fs::write(&conflict_path, "disk line\n").expect("external conflict edit");
        let conflicted = runtime
            .merge_recovery(
                &conflict_checkpoint.recovery_id,
                conflict_checkpoint.recovery_revision,
                &conflict_opened.resource_id,
                "main",
                &conflict_opened.resource_id,
                &conflict_opened.subscription_id,
            )
            .await
            .expect("conflicted merge outcome");
        match conflicted {
            FileRecoveryMergeResultV1::Conflicted {
                disk_changed,
                merged_text,
                ..
            } => {
                assert!(disk_changed);
                assert!(merged_text.contains("<<<<<<<"));
                assert!(merged_text.contains("buffer line"));
                assert!(merged_text.contains("======="));
                assert!(merged_text.contains("disk line"));
                assert!(merged_text.contains(">>>>>>>"));
            }
            other => panic!("expected conflicted merge, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(conflict_path).expect("disk after conflict merge"),
            "disk line\n"
        );
    }

    #[tokio::test]
    async fn file_recovery_merge_requires_new_live_authorization_after_restart() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "buffer",
            )
            .await
            .expect("checkpoint");
        runtime.revoke_test_agent_config("agent-a");

        let restored = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect("stored bytes remain readable");
        assert_eq!(restored.buffer, "buffer");
        let updated = runtime
            .checkpoint_recovery(
                Some(&checkpoint.recovery_id),
                Some(checkpoint.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "newer buffer after revocation",
            )
            .await
            .expect("scoped recovery CAS update does not require current file authority");
        assert_eq!(
            updated
                .file_authorization_error
                .as_ref()
                .map(FileResourceErrorV1::code),
            Some("unauthorized_path")
        );
        assert_eq!(
            runtime
                .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
                .await
                .expect("updated recovery remains readable")
                .buffer,
            "newer buffer after revocation"
        );
        let create = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "new recovery must still require live authority",
            )
            .await
            .expect_err("first recovery checkpoint must retain live authority requirement");
        assert_eq!(create.code(), "unauthorized_path");
        let merge = runtime
            .merge_recovery(
                &checkpoint.recovery_id,
                updated.recovery_revision,
                &opened.resource_id,
                "main",
                &opened.resource_id,
                &opened.subscription_id,
            )
            .await
            .expect_err("revoked subscription must not read disk for merge");
        assert_eq!(merge.code(), "unauthorized_path");
        let save = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "recovery must not authorize this write",
            )
            .await
            .expect_err("recovery must not revive revoked file authority");
        assert_eq!(save.code(), "unauthorized_path");
        assert_eq!(fs::read_to_string(path).expect("disk bytes"), "base");
    }

    #[tokio::test]
    async fn file_recovery_existing_cas_survives_last_subscription_close() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "buffer",
            )
            .await
            .expect("checkpoint");
        runtime
            .close(&opened.subscription_id)
            .await
            .expect("close last subscription");
        assert_eq!(runtime.subscriber_count(&opened.resource_id).await, 0);

        let updated = runtime
            .checkpoint_recovery(
                Some(&checkpoint.recovery_id),
                Some(checkpoint.recovery_revision),
                "closed-resource-does-not-authorize-recovery",
                "closed-subscription-does-not-authorize-recovery",
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "newer buffer after close",
            )
            .await
            .expect("scoped recovery CAS remains independent of a live subscription");
        assert_eq!(updated.recovery_revision, checkpoint.recovery_revision + 1);
        assert_eq!(
            updated
                .file_authorization_error
                .as_ref()
                .map(FileResourceErrorV1::code),
            Some("resource_not_found")
        );
        assert_eq!(
            runtime
                .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
                .await
                .expect("updated recovery")
                .buffer,
            "newer buffer after close"
        );

        let create = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "new authority is forbidden",
            )
            .await
            .expect_err("new recovery still requires an open resource");
        assert_eq!(create.code(), "resource_not_found");
        assert_eq!(fs::read_to_string(path).expect("disk bytes"), "base");
    }

    #[tokio::test]
    async fn file_recovery_rejects_oversized_and_tampered_bodies_without_path_escape() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let oversized = "x".repeat(
            usize::try_from(FileResourceLimits::default().monaco_max_size_bytes)
                .expect("limit fits usize")
                + 1,
        );
        let too_large = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                &oversized,
            )
            .await
            .expect_err("oversized recovery buffer must fail");
        assert_eq!(too_large.code(), "file_too_large");

        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "buffer",
            )
            .await
            .expect("checkpoint");
        let mut manifest =
            load_recovery_manifest(&recovery_root, &checkpoint.recovery_id).expect("manifest");
        let buffer_path = recovery_root
            .join(&checkpoint.recovery_id)
            .join("blobs")
            .join(&manifest.buffer_blob);
        fs::write(&buffer_path, [0xff, 0xfe]).expect("corrupt blob");
        assert_eq!(
            runtime
                .list_recoveries(&opened.resource_id, "main")
                .await
                .expect("discovery validates metadata without reading every body")
                .len(),
            1
        );
        let invalid_utf8 = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("invalid UTF-8 recovery blob must fail");
        assert_eq!(invalid_utf8.code(), "invalid_recovery");

        let secret = temp.path().join("secret.txt");
        fs::write(&secret, "must not be exposed").expect("secret fixture");
        manifest.buffer_blob = "../../secret.txt".to_string();
        wardian_core::conversations::write_json_atomic(
            &recovery_root
                .join(&checkpoint.recovery_id)
                .join("manifest.json"),
            &manifest,
        )
        .expect("tamper manifest");
        let escaped = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("tampered blob path must not escape recovery record");
        assert_eq!(escaped.code(), "invalid_recovery");
        assert_eq!(
            fs::read_to_string(secret).expect("secret bytes"),
            "must not be exposed"
        );
    }

    #[test]
    fn file_recovery_rejects_non_file_preexisting_hash_blob() {
        let temp = tempfile::tempdir().expect("temp root");
        let record_dir = temp.path().join("recovery-id");
        let blobs_dir = record_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).expect("blob directory");
        fs::create_dir(blobs_dir.join(recovery_blob_name("buffer")))
            .expect("non-file blob fixture");

        let failure = write_recovery_blob(&record_dir, "buffer")
            .expect_err("non-file hash blob must fail closed");
        assert_eq!(failure.code(), "invalid_recovery");
    }

    #[tokio::test]
    async fn file_recovery_is_cleaned_after_successful_guarded_save() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "saved buffer",
            )
            .await
            .expect("checkpoint");

        let saved = runtime
            .save_text_with_recovery_cleanup(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "saved buffer",
                Some(&FileRecoveryCleanupV1 {
                    recovery_id: checkpoint.recovery_id.clone(),
                    expected_recovery_revision: checkpoint.recovery_revision,
                }),
                "main",
            )
            .await
            .expect("save");
        assert!(matches!(saved, FileResourceSaveResultV1::Saved { .. }));
        let recovery = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("successful save must clean recovery");
        assert_eq!(recovery.code(), "recovery_not_found");
        assert_eq!(
            fs::read_to_string(path).expect("saved bytes"),
            "saved buffer"
        );
    }

    #[tokio::test]
    async fn file_resources_save_text_is_guarded_and_emits_one_logical_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let base_hash = opened.descriptor.content_hash.clone();
        let mut events = runtime.subscribe_events();

        let saved = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &base_hash,
                "revision two",
            )
            .await
            .expect("save");
        let (saved_revision, saved_hash) = match saved {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved result, got {other:?}"),
        };
        assert_eq!(saved_revision, opened.revision + 1);
        assert_ne!(saved_hash, base_hash);
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("save event timeout")
            .expect("save event");
        assert_eq!(event.revision, saved_revision);
        assert_eq!(event.descriptor.content_hash, saved_hash);

        let stale = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &base_hash,
                "stale overwrite",
            )
            .await
            .expect("stale conflict result");
        assert_eq!(
            stale,
            FileResourceSaveResultV1::StaleConflict {
                revision: saved_revision,
                content_hash: saved_hash,
            }
        );
        assert_eq!(
            fs::read_to_string(&path).expect("saved bytes"),
            "revision two"
        );
        assert!(timeout(Duration::from_millis(400), events.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn file_resources_save_text_rebinds_every_live_subscription() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared-draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");

        let first_save = runtime
            .save_text(
                &first.resource_id,
                &first.subscription_id,
                first.revision,
                &first.descriptor.content_hash,
                "revision two",
            )
            .await
            .expect("first subscription save");
        let (second_revision, second_hash) = match first_save {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        assert_eq!(
            runtime
                .read_text(
                    &second.resource_id,
                    &second.subscription_id,
                    second_revision,
                    Some(&config),
                )
                .await
                .expect("second subscription reads rebound identity")
                .text,
            "revision two"
        );

        let second_save = runtime
            .save_text(
                &second.resource_id,
                &second.subscription_id,
                second_revision,
                &second_hash,
                "revision three",
            )
            .await
            .expect("second subscription saves rebound identity");
        let third_revision = match second_save {
            FileResourceSaveResultV1::Saved { revision, .. } => revision,
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        assert_eq!(
            runtime
                .read_text(
                    &first.resource_id,
                    &first.subscription_id,
                    third_revision,
                    Some(&config),
                )
                .await
                .expect("first subscription reads second rebound identity")
                .text,
            "revision three"
        );
    }

    #[tokio::test]
    async fn file_resources_save_serializes_concurrent_subscription_admission() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("concurrent-draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let hook = SaveAfterValidationHook {
            validation_reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume_save: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.save_after_validation_hook.lock().await = Some(hook.clone());

        let save_runtime = runtime.clone();
        let save_resource_id = first.resource_id.clone();
        let save_subscription_id = first.subscription_id.clone();
        let save_hash = first.descriptor.content_hash.clone();
        let save = tokio::spawn(async move {
            save_runtime
                .save_text(
                    &save_resource_id,
                    &save_subscription_id,
                    first.revision,
                    &save_hash,
                    "revision two",
                )
                .await
        });
        hook.validation_reached.wait().await;

        let open_runtime = runtime.clone();
        let open_config = config.clone();
        let open_path = path.clone();
        let mut concurrent_open = tokio::spawn(async move {
            open_runtime
                .open_agent_file("agent-a", &open_config, &open_path, None)
                .await
        });
        assert!(
            timeout(Duration::from_millis(100), &mut concurrent_open)
                .await
                .is_err(),
            "existing-resource admission must wait for the save operation"
        );
        *runtime.inner.save_after_validation_hook.lock().await = None;
        hook.resume_save.wait().await;

        let saved = save.await.expect("save task").expect("save result");
        let (saved_revision, saved_hash) = match saved {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        let second = concurrent_open
            .await
            .expect("open task")
            .expect("concurrent open");
        assert_eq!(second.revision, saved_revision);
        assert_eq!(
            runtime
                .read_text(
                    &second.resource_id,
                    &second.subscription_id,
                    saved_revision,
                    Some(&config),
                )
                .await
                .expect("concurrent subscription reads replacement")
                .text,
            "revision two"
        );

        let saved_again = runtime
            .save_text(
                &second.resource_id,
                &second.subscription_id,
                saved_revision,
                &saved_hash,
                "revision three",
            )
            .await
            .expect("concurrent subscription saves replacement");
        let final_revision = match saved_again {
            FileResourceSaveResultV1::Saved { revision, .. } => revision,
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        assert_eq!(
            runtime
                .read_text(
                    &first.resource_id,
                    &first.subscription_id,
                    final_revision,
                    Some(&config),
                )
                .await
                .expect("original subscription reads second replacement")
                .text,
            "revision three"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_consumes_one_exact_target_grant() {
        let temp = tempfile::tempdir().expect("temp root");
        let selected = temp.path().join("selected.txt");
        let sibling = temp.path().join("sibling.txt");
        let runtime = test_runtime();
        let grant = runtime
            .record_save_target(&selected)
            .await
            .expect("save target grant");

        let saved = runtime
            .save_file_resource_as_text(&grant.save_target_grant_id, "saved text")
            .await
            .expect("save as");

        assert_eq!(saved.canonical_path, grant.selected_path);
        assert_eq!(saved.resource_id, file_resource_id(&grant.selected_path));
        assert_eq!(
            fs::read_to_string(&selected).expect("selected bytes"),
            "saved text"
        );
        let opened = runtime
            .open_user_file(&saved.capability_id, &selected, None)
            .await
            .expect("published Save As capability opens its exact target");
        runtime
            .close(&opened.subscription_id)
            .await
            .expect("close saved target");
        assert!(
            !sibling.exists(),
            "exact grant must not create a sibling name"
        );
        assert_eq!(
            runtime
                .save_file_resource_as_text(&grant.save_target_grant_id, "second use")
                .await
                .expect_err("save target grant must be one-shot")
                .code(),
            "unauthorized_save_target"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_reserves_capacity_before_creating_missing_target() {
        let temp = tempfile::tempdir().expect("temp root");
        let occupied_path = temp.path().join("occupied.txt");
        let selected = temp.path().join("missing-target.txt");
        fs::write(&occupied_path, "occupied").expect("occupied fixture");
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            1,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let occupied_grant = runtime
            .record_user_file(&occupied_path)
            .await
            .expect("occupied capability");
        let occupied = runtime
            .open_user_file(&occupied_grant.capability_id, &occupied_path, None)
            .await
            .expect("active occupied capability");
        let save_target = runtime
            .record_save_target(&selected)
            .await
            .expect("missing save target");

        assert_eq!(
            runtime
                .save_file_resource_as_text(&save_target.save_target_grant_id, "submitted")
                .await
                .expect_err("active capacity must reject before create")
                .code(),
            "grant_limit_reached"
        );
        assert!(!selected.exists(), "capacity error must not create target");

        runtime
            .close(&occupied.subscription_id)
            .await
            .expect("release occupied capability");
        runtime
            .save_file_resource_as_text(&save_target.save_target_grant_id, "submitted")
            .await
            .expect("capacity rejection must not consume target grant");
        assert_eq!(
            fs::read_to_string(&selected).expect("saved target"),
            "submitted"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_reserves_capacity_before_replacing_existing_target() {
        let temp = tempfile::tempdir().expect("temp root");
        let occupied_path = temp.path().join("occupied.txt");
        let selected = temp.path().join("existing-target.txt");
        fs::write(&occupied_path, "occupied").expect("occupied fixture");
        fs::write(&selected, "original target bytes").expect("existing fixture");
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            1,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let occupied_grant = runtime
            .record_user_file(&occupied_path)
            .await
            .expect("occupied capability");
        let _occupied = runtime
            .open_user_file(&occupied_grant.capability_id, &occupied_path, None)
            .await
            .expect("active occupied capability");
        let save_target = runtime
            .record_save_target(&selected)
            .await
            .expect("existing save target");

        assert_eq!(
            runtime
                .save_file_resource_as_text(&save_target.save_target_grant_id, "submitted")
                .await
                .expect_err("active capacity must reject before replace")
                .code(),
            "grant_limit_reached"
        );
        assert_eq!(
            fs::read_to_string(&selected).expect("unchanged existing target"),
            "original target bytes"
        );
    }

    #[tokio::test]
    async fn file_resources_save_text_reports_unchanged_and_refreshes_external_stale_conflict() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let base_hash = opened.descriptor.content_hash.clone();
        let mut events = runtime.subscribe_events();

        assert_eq!(
            runtime
                .save_text(
                    &opened.resource_id,
                    &opened.subscription_id,
                    opened.revision,
                    &base_hash,
                    "revision one",
                )
                .await
                .expect("unchanged save"),
            FileResourceSaveResultV1::Unchanged {
                revision: opened.revision,
                content_hash: base_hash.clone(),
            }
        );
        assert!(timeout(Duration::from_millis(250), events.recv())
            .await
            .is_err());

        fs::write(&path, "external revision").expect("external mutation");
        let conflict = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &base_hash,
                "must not overwrite",
            )
            .await
            .expect("tagged stale conflict");
        let (revision, content_hash) = match conflict {
            FileResourceSaveResultV1::StaleConflict {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected stale conflict, got {other:?}"),
        };
        assert_eq!(revision, opened.revision + 1);
        assert_ne!(content_hash, base_hash);
        assert_eq!(
            fs::read_to_string(&path).expect("external bytes"),
            "external revision"
        );
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("refresh event timeout")
            .expect("refresh event");
        assert_eq!(event.revision, revision);
        assert_eq!(event.descriptor.content_hash, content_hash);
        assert!(timeout(Duration::from_millis(400), events.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn file_resources_save_text_rejects_revoked_roots_and_changed_identity() {
        let temp = tempfile::tempdir().expect("temp root");
        let authorized_root = temp.path().join("authorized");
        let revoked_root = temp.path().join("revoked");
        fs::create_dir_all(&authorized_root).expect("authorized root");
        fs::create_dir_all(&revoked_root).expect("revoked root");
        let path = authorized_root.join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", &authorized_root);
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let revoked = agent_config("agent-a", &revoked_root);
        runtime
            .current_agent_config_resolver()
            .observe_open("agent-a", &revoked);
        assert_eq!(
            runtime
                .save_text(
                    &opened.resource_id,
                    &opened.subscription_id,
                    opened.revision,
                    &opened.descriptor.content_hash,
                    "must not save",
                )
                .await
                .expect_err("revoked root must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("original bytes"),
            "revision one"
        );

        runtime
            .current_agent_config_resolver()
            .observe_open("agent-a", &config);
        let replacement = authorized_root.join("replacement.txt");
        fs::write(&replacement, "replacement identity").expect("replacement fixture");
        replace_path_identity(&replacement, &path);
        assert_eq!(
            runtime
                .save_text(
                    &opened.resource_id,
                    &opened.subscription_id,
                    opened.revision,
                    &opened.descriptor.content_hash,
                    "must not overwrite replacement",
                )
                .await
                .expect_err("changed identity must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("replacement bytes"),
            "replacement identity"
        );
    }

    #[tokio::test]
    async fn file_resources_save_revalidates_backend_claim_after_initial_validation() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("revoked-during-save.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let mut events = runtime.subscribe_events();
        let hook = SaveAfterValidationHook {
            validation_reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume_save: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.save_after_validation_hook.lock().await = Some(hook.clone());

        let save_runtime = runtime.clone();
        let resource_id = opened.resource_id.clone();
        let subscription_id = opened.subscription_id.clone();
        let content_hash = opened.descriptor.content_hash.clone();
        let save = tokio::spawn(async move {
            save_runtime
                .save_text(
                    &resource_id,
                    &subscription_id,
                    opened.revision,
                    &content_hash,
                    "must not commit",
                )
                .await
        });
        hook.validation_reached.wait().await;
        runtime.revoke_test_agent_config("agent-a");
        *runtime.inner.save_after_validation_hook.lock().await = None;
        hook.resume_save.wait().await;

        assert_eq!(
            save.await
                .expect("save task")
                .expect_err("commit-time revoked claim must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("original bytes"),
            "revision one"
        );
        assert!(
            timeout(Duration::from_millis(400), events.recv())
                .await
                .is_err(),
            "rejected save must not emit a saved event"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_fails_closed_when_target_or_parent_binding_changes() {
        let temp = tempfile::tempdir().expect("temp root");
        let runtime = test_runtime();

        let missing = temp.path().join("missing.txt");
        let missing_grant = runtime
            .record_save_target(&missing)
            .await
            .expect("missing target grant");
        fs::write(&missing, "attacker bytes").expect("target race");
        assert_eq!(
            runtime
                .save_file_resource_as_text(&missing_grant.save_target_grant_id, "submitted")
                .await
                .expect_err("new target binding must fail")
                .code(),
            "unauthorized_save_target"
        );
        assert_eq!(
            fs::read_to_string(&missing).expect("attacker bytes"),
            "attacker bytes"
        );

        let existing = temp.path().join("existing.txt");
        fs::write(&existing, "original").expect("existing fixture");
        let existing_grant = runtime
            .record_save_target(&existing)
            .await
            .expect("existing target grant");
        let replacement = temp.path().join("existing-replacement.txt");
        fs::write(&replacement, "replacement identity").expect("replacement fixture");
        replace_path_identity(&replacement, &existing);
        assert_eq!(
            runtime
                .save_file_resource_as_text(&existing_grant.save_target_grant_id, "submitted")
                .await
                .expect_err("existing target identity change must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&existing).expect("replacement bytes"),
            "replacement identity"
        );

        let approved_parent = temp.path().join("approved-parent");
        let other_parent = temp.path().join("other-parent");
        fs::create_dir_all(&approved_parent).expect("approved parent");
        fs::create_dir_all(&other_parent).expect("other parent");
        let alias_parent = temp.path().join("selected-parent");
        create_directory_link(&approved_parent, &alias_parent);
        let alias_target = alias_parent.join("copy.txt");
        let parent_grant = runtime
            .record_save_target(&alias_target)
            .await
            .expect("parent-bound grant");
        remove_directory_link(&alias_parent);
        create_directory_link(&other_parent, &alias_parent);
        assert_eq!(
            runtime
                .save_file_resource_as_text(&parent_grant.save_target_grant_id, "submitted")
                .await
                .expect_err("retargeted parent must fail")
                .code(),
            "unauthorized_save_target"
        );
        assert!(!approved_parent.join("copy.txt").exists());
        assert!(!other_parent.join("copy.txt").exists());
    }

    #[tokio::test]
    async fn file_resources_save_as_never_retargets_the_open_source_resource() {
        let temp = tempfile::tempdir().expect("temp root");
        let source = temp.path().join("source.txt");
        let copy = temp.path().join("copy.txt");
        fs::write(&source, "source bytes").expect("source fixture");
        fs::write(&copy, "old copy bytes").expect("existing copy fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &source, None)
            .await
            .expect("open source");
        let grant = runtime.record_save_target(&copy).await.expect("copy grant");

        let saved = runtime
            .save_file_resource_as_text(&grant.save_target_grant_id, "copy bytes")
            .await
            .expect("save copy");

        assert_ne!(saved.resource_id, opened.resource_id);
        let source_snapshot = runtime
            .snapshot(&opened.resource_id)
            .await
            .expect("source remains open");
        assert_eq!(source_snapshot.resource_id, opened.resource_id);
        assert_eq!(source_snapshot.subscription_id, opened.subscription_id);
        assert_eq!(
            fs::read_to_string(&source).expect("source bytes"),
            "source bytes"
        );
        assert_eq!(fs::read_to_string(&copy).expect("copy bytes"), "copy bytes");
    }

    #[tokio::test]
    async fn subscribers_share_one_watcher_and_close_by_reference_count() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared.txt");
        fs::write(&path, "one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();

        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");

        assert_eq!(first.resource_id, second.resource_id);
        assert_ne!(first.subscription_id, second.subscription_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&first.resource_id).await, 2);

        runtime
            .close(&first.subscription_id)
            .await
            .expect("close first");
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&first.resource_id).await, 1);

        runtime
            .close(&second.subscription_id)
            .await
            .expect("close second");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn removed_alias_only_revokes_its_subscription_while_direct_text_refreshes() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();

        let alias = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open through alias");
        let direct = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("open directly");
        assert_eq!(alias.resource_id, direct.resource_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&alias.resource_id).await, 2);

        remove_directory_link(&alias_dir);
        assert_eq!(
            runtime
                .read_text(
                    &alias.resource_id,
                    &alias.subscription_id,
                    alias.revision,
                    Some(&config),
                )
                .await
                .expect_err("removed alias must revoke only its subscription")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    direct.revision,
                    Some(&config),
                )
                .await
                .expect("direct subscription remains readable")
                .text,
            "revision one\n"
        );

        let mut events = runtime.subscribe_events();
        fs::write(&canonical_path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(direct.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("direct refresh timeout")
            .expect("direct refresh event");
        assert_eq!(event.revision, direct.revision + 1);
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect("refreshed direct read")
                .text,
            "revision two\n"
        );
        assert_eq!(
            runtime
                .read_text(
                    &alias.resource_id,
                    &alias.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect_err("removed alias stays revoked after shared refresh")
                .code(),
            "unauthorized_path"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias subscription");
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&direct.resource_id).await, 1);
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect("direct read survives alias close")
                .text,
            "revision two\n"
        );
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct subscription");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn valid_direct_join_recovers_alias_only_unavailable_without_file_event() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let alias = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open alias");

        remove_directory_link(&alias_dir);
        runtime.schedule_refresh(alias.resource_id.clone());
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("alias unavailable timeout")
            .expect("alias unavailable event");
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );

        let direct = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("join valid direct subscription");
        assert_eq!(direct.revision, unavailable.revision);
        assert_eq!(
            direct.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("membership recovery timeout")
            .expect("membership recovery event");
        assert_eq!(recovered.revision, unavailable.revision + 1);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    recovered.revision,
                    Some(&config),
                )
                .await
                .expect("direct subscription reads recovered revision")
                .text,
            "stable\n"
        );
        sleep(Duration::from_millis(500)).await;
        assert!(
            events.try_recv().is_err(),
            "membership recovery must not schedule an infinite refresh loop"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias");
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct");
    }

    #[tokio::test]
    async fn closing_last_valid_candidate_marks_invalid_only_resource_unavailable_once() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let alias = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open alias");
        let direct = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("open direct");
        let original_hash = direct.descriptor.content_hash.clone();
        let mut events = runtime.subscribe_events();

        remove_directory_link(&alias_dir);
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close last valid candidate");
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("membership unavailable timeout")
            .expect("membership unavailable event");
        assert_eq!(unavailable.revision, direct.revision + 1);
        assert_eq!(unavailable.descriptor.content_hash, original_hash);
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        sleep(Duration::from_millis(500)).await;
        assert!(
            events.try_recv().is_err(),
            "invalid-only membership must settle after one unavailable revision"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close invalid alias");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn revoked_agent_candidate_is_skipped_before_valid_picker_refresh_scan() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-agent");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let agent = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open agent alias");
        let grant = runtime
            .record_user_file(&canonical_path)
            .await
            .expect("picker grant");
        let picker = runtime
            .open_user_file(&grant.capability_id, &canonical_path, None)
            .await
            .expect("open picker direct");
        let initial_hash = picker.descriptor.content_hash.clone();
        runtime.revoke_test_agent_config("agent-a");
        let scans_before = runtime.inner.refresh_scan_count.load(Ordering::Acquire);
        let mut events = runtime.subscribe_events();

        fs::write(&canonical_path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(picker.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("picker fallback timeout")
            .expect("picker fallback event");
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_ne!(event.descriptor.content_hash, initial_hash);
        assert_eq!(
            runtime.inner.refresh_scan_count.load(Ordering::Acquire),
            scans_before + 1,
            "revoked agent candidate must be rejected before descriptor scanning"
        );
        assert_eq!(
            runtime
                .read_text(
                    &picker.resource_id,
                    &picker.subscription_id,
                    event.revision,
                    None,
                )
                .await
                .expect("picker reads refreshed revision")
                .text,
            "revision two\n"
        );

        runtime
            .close(&agent.subscription_id)
            .await
            .expect("close revoked agent");
        runtime
            .close(&picker.subscription_id)
            .await
            .expect("close picker");
    }

    #[tokio::test]
    async fn revoked_picker_candidate_is_skipped_before_valid_agent_refresh_scan() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-picker");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let grant = runtime
            .record_user_file(&alias_path)
            .await
            .expect("picker grant");
        let picker = runtime
            .open_user_file(&grant.capability_id, &alias_path, None)
            .await
            .expect("open picker alias");
        let agent = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("open agent direct");
        let initial_hash = agent.descriptor.content_hash.clone();
        runtime
            .inner
            .user_file_grants
            .lock()
            .await
            .remove(&grant.capability_id);
        let scans_before = runtime.inner.refresh_scan_count.load(Ordering::Acquire);
        let mut events = runtime.subscribe_events();

        fs::write(&canonical_path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(agent.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("agent fallback timeout")
            .expect("agent fallback event");
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_ne!(event.descriptor.content_hash, initial_hash);
        assert_eq!(
            runtime.inner.refresh_scan_count.load(Ordering::Acquire),
            scans_before + 1,
            "revoked picker candidate must be rejected before descriptor scanning"
        );

        runtime
            .close(&picker.subscription_id)
            .await
            .expect("close revoked picker");
        runtime
            .close(&agent.subscription_id)
            .await
            .expect("close agent");
    }

    #[tokio::test]
    async fn invalid_only_live_claim_preserves_prior_hash_without_descriptor_scan() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared.txt");
        fs::write(&path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let agent = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open agent file");
        let initial_hash = agent.descriptor.content_hash.clone();
        runtime.revoke_test_agent_config("agent-a");
        let scans_before = runtime.inner.refresh_scan_count.load(Ordering::Acquire);
        let mut events = runtime.subscribe_events();

        fs::write(&path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(agent.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("invalid-only timeout")
            .expect("invalid-only event");
        assert_eq!(event.descriptor.content_hash, initial_hash);
        assert_eq!(
            event.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        assert_eq!(
            runtime.inner.refresh_scan_count.load(Ordering::Acquire),
            scans_before,
            "invalid-only authority must not scan or publish the changed hash"
        );
        sleep(Duration::from_millis(500)).await;
        assert!(
            events.try_recv().is_err(),
            "invalid-only refresh must settle"
        );

        runtime
            .close(&agent.subscription_id)
            .await
            .expect("close revoked agent");
    }

    #[tokio::test]
    async fn retargeted_picker_alias_cannot_poison_direct_pdf_ticket_or_atomic_refresh() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let other_dir = temp.path().join("y-other");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        fs::create_dir(&other_dir).expect("other directory");
        let canonical_path = canonical_dir.join("shared.pdf");
        let other_path = other_dir.join("shared.pdf");
        fs::write(&canonical_path, b"%PDF-1.7 revision one").expect("fixture");
        fs::write(&other_path, b"%PDF-1.7 unrelated target").expect("other fixture");
        create_directory_link(&canonical_dir, &alias_dir);
        let alias_path = alias_dir.join("shared.pdf");
        let runtime = test_runtime();

        let alias_grant = runtime
            .record_user_file(&alias_path)
            .await
            .expect("alias picker grant");
        let alias = runtime
            .open_user_file(&alias_grant.capability_id, &alias_path, None)
            .await
            .expect("open picker alias");
        let direct_grant = runtime
            .record_user_file(&canonical_path)
            .await
            .expect("direct picker grant");
        assert_eq!(
            alias_grant.capability_id, direct_grant.capability_id,
            "exact grants deduplicate by canonical resource without widening a subscription"
        );
        let direct = runtime
            .open_user_file(&direct_grant.capability_id, &canonical_path, None)
            .await
            .expect("open picker path directly");
        let alias_after_dedup = runtime
            .open_user_file(&direct_grant.capability_id, &alias_path, None)
            .await
            .expect("deduplicated grant still retains this open's alias provenance");
        assert_eq!(alias.resource_id, direct.resource_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&direct.resource_id).await, 3);

        remove_directory_link(&alias_dir);
        create_directory_link(&other_dir, &alias_dir);
        assert_eq!(
            runtime
                .issue_ticket(
                    &alias.resource_id,
                    &alias.subscription_id,
                    alias.revision,
                    None,
                    "alias-before-refresh",
                )
                .await
                .expect_err("retargeted alias must not mint a ticket")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &alias_after_dedup.resource_id,
                    &alias_after_dedup.subscription_id,
                    alias_after_dedup.revision,
                    None,
                    "deduplicated-alias-before-refresh",
                )
                .await
                .expect_err("deduplicated capability must not replace alias provenance")
                .code(),
            "unauthorized_path"
        );
        let direct_ticket = runtime
            .issue_ticket(
                &direct.resource_id,
                &direct.subscription_id,
                direct.revision,
                None,
                "direct-before-refresh",
            )
            .await
            .expect("direct ticket survives alias retarget");
        assert_eq!(
            runtime
                .read_ticket_range(&direct_ticket.ticket_id, None)
                .await
                .expect("read direct ticket")
                .bytes,
            b"%PDF-1.7 revision one"
        );

        let replacement = canonical_dir.join("shared.replacement");
        fs::write(&replacement, b"%PDF-1.7 revision two").expect("replacement fixture");
        replace_path_identity(&replacement, &canonical_path);
        let mut events = runtime.subscribe_events();
        runtime.schedule_refresh(direct.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("atomic refresh timeout")
            .expect("atomic refresh event");
        assert_eq!(event.revision, direct.revision + 1);
        assert_eq!(event.descriptor.unavailable_reason, None);

        let refreshed_ticket = runtime
            .issue_ticket(
                &direct.resource_id,
                &direct.subscription_id,
                event.revision,
                None,
                "direct-after-refresh",
            )
            .await
            .expect("direct ticket survives atomic replacement");
        assert_eq!(
            runtime
                .read_ticket_range(&refreshed_ticket.ticket_id, None)
                .await
                .expect("read refreshed ticket")
                .bytes,
            b"%PDF-1.7 revision two"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &alias.resource_id,
                    &alias.subscription_id,
                    event.revision,
                    None,
                    "alias-after-refresh",
                )
                .await
                .expect_err("retargeted alias remains revoked after direct refresh")
                .code(),
            "unauthorized_path"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias subscription");
        assert_eq!(runtime.watcher_count().await, 1);
        runtime
            .close(&alias_after_dedup.subscription_id)
            .await
            .expect("close deduplicated alias subscription");
        assert_eq!(runtime.watcher_count().await, 1);
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct subscription");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_alias_and_direct_first_opens_retain_both_authorizations() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "concurrent\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first_open_barrier = Arc::new(tokio::sync::Barrier::new(2));
        *runtime.inner.open_after_entry_miss_hook.lock().await = Some(first_open_barrier.clone());

        let alias_task = {
            let runtime = runtime.clone();
            let config = config.clone();
            let alias_path = alias_path.clone();
            tokio::spawn(async move {
                runtime
                    .open_agent_file("agent-a", &config, &alias_path, None)
                    .await
            })
        };
        let direct_task = {
            let runtime = runtime.clone();
            let config = config.clone();
            let canonical_path = canonical_path.clone();
            tokio::spawn(async move {
                runtime
                    .open_agent_file("agent-a", &config, &canonical_path, None)
                    .await
            })
        };
        let (alias_result, direct_result) = timeout(Duration::from_secs(5), async {
            tokio::join!(alias_task, direct_task)
        })
        .await
        .expect("concurrent opens must not deadlock");
        let alias = alias_result
            .expect("alias task")
            .expect("concurrent alias open");
        let direct = direct_result
            .expect("direct task")
            .expect("concurrent direct open");
        *runtime.inner.open_after_entry_miss_hook.lock().await = None;

        assert_eq!(alias.resource_id, direct.resource_id);
        assert_ne!(alias.subscription_id, direct.subscription_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&direct.resource_id).await, 2);

        remove_directory_link(&alias_dir);
        assert_eq!(
            runtime
                .read_text(
                    &alias.resource_id,
                    &alias.subscription_id,
                    alias.revision,
                    Some(&config),
                )
                .await
                .expect_err("concurrent alias subscription retained its own provenance")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    direct.revision,
                    Some(&config),
                )
                .await
                .expect("concurrent direct subscription retained its own provenance")
                .text,
            "concurrent\n"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias subscription");
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct subscription");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn second_open_during_a_write_burst_returns_the_stored_stable_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("second-open.txt");
        fs::write(&path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");

        fs::write(&path, "intermediate\n").expect("intermediate write");
        runtime.schedule_refresh(first.resource_id.clone());
        sleep(Duration::from_millis(50)).await;
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");

        assert_eq!(second.revision, first.revision);
        assert_eq!(
            second.descriptor.content_hash,
            first.descriptor.content_hash
        );
        assert!(
            events.try_recv().is_err(),
            "second open promoted an unstable revision"
        );

        fs::write(&path, "stable after burst\n").expect("final write");
        runtime.schedule_refresh(first.resource_id.clone());
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("stable event timeout")
            .expect("stable event");
        assert_eq!(event.revision, first.revision + 1);
    }

    #[tokio::test]
    async fn coalesces_write_bursts_into_one_stable_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("burst.txt");
        fs::write(&path, "initial\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        tokio::fs::write(&path, "first\n").await.expect("write one");
        tokio::fs::write(&path, "second\n")
            .await
            .expect("write two");
        tokio::fs::write(&path, "third\n")
            .await
            .expect("write three");

        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("stable event timeout")
            .expect("stable event");
        assert_eq!(event.resource_id, subscription.resource_id);
        assert_eq!(event.revision, 2);
        sleep(Duration::from_millis(250)).await;
        assert!(events.try_recv().is_err(), "raw notify burst leaked");
    }

    #[tokio::test]
    async fn same_path_identity_replacement_advances_once_and_refreshes_picker_grant() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("atomic.txt");
        fs::write(&path, "revision one\n").expect("fixture");
        let runtime = test_runtime();
        let grant = runtime.record_user_file(&path).await.expect("picker grant");
        let subscription = runtime
            .open_user_file(&grant.capability_id, &path, None)
            .await
            .expect("open picker file");
        let mut events = runtime.subscribe_events();

        let replacement = temp.path().join("atomic.replacement");
        fs::write(&replacement, "revision two\n").expect("replacement fixture");
        replace_path_identity(&replacement, &path);
        runtime.schedule_refresh(subscription.resource_id.clone());

        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("replacement event timeout")
            .expect("replacement event");
        assert_eq!(event.revision, subscription.revision + 1);
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    event.revision,
                    None,
                )
                .await
                .expect("replacement read")
                .text,
            "revision two\n"
        );
        sleep(Duration::from_millis(300)).await;
        assert!(events.try_recv().is_err(), "replacement emitted twice");

        let refreshed_grant = runtime
            .inner
            .user_file_grants
            .lock()
            .await
            .get(&grant.capability_id)
            .expect("live grant")
            .authorized
            .clone();
        verified_snapshot(refreshed_grant, runtime.inner.limits.clone())
            .await
            .expect("grant retains the replacement identity");
    }

    #[tokio::test]
    async fn persistent_refresh_failure_is_typed_once_and_recovers() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("unavailable.txt");
        let moved = temp.path().join("unavailable.moved");
        fs::write(&path, "stable\n").expect("fixture");
        let runtime = test_runtime();
        let grant = runtime.record_user_file(&path).await.expect("picker grant");
        let subscription = runtime
            .open_user_file(&grant.capability_id, &path, None)
            .await
            .expect("open");
        let mut events = runtime.subscribe_events();

        fs::rename(&path, &moved).expect("move file away");
        runtime.schedule_refresh(subscription.resource_id.clone());
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("unavailable event timeout")
            .expect("unavailable event");
        assert_eq!(unavailable.revision, 2);
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        assert!(!unavailable.descriptor.capabilities.preview);
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    None,
                )
                .await
                .expect_err("unavailable text revision must not read the old handle")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    None,
                    "unavailable-lease",
                )
                .await
                .expect_err("unavailable revision must not mint a stream ticket")
                .code(),
            "unauthorized_path"
        );

        runtime.schedule_refresh(subscription.resource_id.clone());
        sleep(Duration::from_millis(300)).await;
        assert!(
            events.try_recv().is_err(),
            "identical failure state repeated"
        );

        fs::rename(&moved, &path).expect("restore original identity");
        runtime.schedule_refresh(subscription.resource_id.clone());
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("recovery event timeout")
            .expect("recovery event");
        assert_eq!(recovered.revision, 3);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
        assert!(recovered.descriptor.capabilities.preview);
    }

    #[tokio::test]
    async fn persistent_unstable_scan_is_typed_once_and_recovers() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("unstable.txt");
        fs::write(&path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let mut events = runtime.subscribe_events();

        *runtime.inner.forced_refresh_error.lock().await = Some(error(
            "unstable_file",
            "file changed during every descriptor scan attempt",
        ));
        runtime.schedule_refresh(subscription.resource_id.clone());
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("unstable event timeout")
            .expect("unstable event");
        assert_eq!(unavailable.revision, 2);
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unstable_file")
        );
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    Some(&config),
                )
                .await
                .expect_err("unstable text revision must reject reads")
                .code(),
            "unstable_file"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    Some(&config),
                    "unstable-lease",
                )
                .await
                .expect_err("unstable revision must reject stream tickets")
                .code(),
            "unstable_file"
        );

        *runtime.inner.forced_refresh_error.lock().await = Some(error(
            "unstable_file",
            "file changed during every descriptor scan attempt",
        ));
        runtime.schedule_refresh(subscription.resource_id.clone());
        sleep(Duration::from_millis(300)).await;
        assert!(
            events.try_recv().is_err(),
            "identical unstable state repeated"
        );

        runtime.schedule_refresh(subscription.resource_id.clone());
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("unstable recovery timeout")
            .expect("unstable recovery event");
        assert_eq!(recovered.revision, 3);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
    }

    #[tokio::test]
    async fn debounce_waits_for_stability_after_the_last_separated_write() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("separated.txt");
        fs::write(&path, "initial\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        for content in ["first\n", "second\n", "third\n"] {
            fs::write(&path, content).expect("write");
            runtime.schedule_refresh(subscription.resource_id.clone());
            sleep(Duration::from_millis(75)).await;
        }

        assert!(
            timeout(Duration::from_millis(60), events.recv())
                .await
                .is_err(),
            "revision arrived before 150 ms of last-write stability"
        );
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("stable event timeout")
            .expect("stable event");
        assert_eq!(event.resource_id, subscription.resource_id);
        assert_eq!(event.revision, 2);
    }

    #[tokio::test]
    async fn old_incarnation_cannot_refresh_a_closed_and_reopened_resource() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("aba.txt");
        fs::write(&path, "first incarnation\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let old_incarnation = runtime
            .inner
            .entries
            .lock()
            .await
            .get(&first.resource_id)
            .expect("first entry")
            .incarnation_id;
        runtime
            .close(&first.subscription_id)
            .await
            .expect("close first");

        fs::write(&path, "second incarnation\n").expect("reopen content");
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");
        fs::write(&path, "unstable replacement\n").expect("unstable content");

        runtime
            .refresh_if_stable(&second.resource_id, old_incarnation, 0)
            .await;
        let current = runtime
            .snapshot(&second.resource_id)
            .await
            .expect("current snapshot");
        assert_eq!(current.revision, 1);
        assert_eq!(
            current.descriptor.content_hash,
            second.descriptor.content_hash
        );
    }

    #[tokio::test]
    async fn unchanged_content_hash_does_not_advance_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("same.txt");
        fs::write(&path, "same bytes\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        tokio::fs::write(&path, "same bytes\n")
            .await
            .expect("rewrite");

        assert!(
            timeout(Duration::from_millis(500), events.recv())
                .await
                .is_err(),
            "unchanged content emitted a revision"
        );
        let current = runtime
            .snapshot(&subscription.resource_id)
            .await
            .expect("snapshot");
        assert_eq!(current.revision, 1);
    }

    #[tokio::test]
    async fn text_reads_are_revision_bound_and_revocation_is_rechecked() {
        let temp = tempfile::tempdir().expect("temp root");
        let allowed = temp.path().join("allowed");
        let revoked = temp.path().join("revoked");
        fs::create_dir_all(&allowed).expect("allowed root");
        fs::create_dir_all(&revoked).expect("revoked root");
        let path = allowed.join("report.txt");
        fs::write(&path, "revision one\n").expect("fixture");
        let initial_config = agent_config("agent-a", &allowed);
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &initial_config, &path, None)
            .await
            .expect("open");

        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    1,
                    Some(&initial_config),
                )
                .await
                .expect("current read")
                .text,
            "revision one\n"
        );

        tokio::fs::write(&path, "revision two\n")
            .await
            .expect("rewrite");
        timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("revision timeout")
            .expect("revision event");

        let stale = runtime
            .read_text(
                &subscription.resource_id,
                &subscription.subscription_id,
                1,
                Some(&initial_config),
            )
            .await
            .expect_err("stale revision must fail");
        assert_eq!(stale.code(), "stale_revision");

        let revoked_config = agent_config("agent-a", &revoked);
        let revoked = runtime
            .read_text(
                &subscription.resource_id,
                &subscription.subscription_id,
                2,
                Some(&revoked_config),
            )
            .await
            .expect_err("revoked root must fail");
        assert_eq!(revoked.code(), "unauthorized_path");
    }

    #[tokio::test]
    async fn oversized_resources_open_as_metadata_and_reject_reads_until_recovery() {
        let temp = tempfile::tempdir().expect("temp root");
        let limits = FileResourceLimits {
            monaco_max_size_bytes: 64,
            monaco_max_line_count: u64::MAX,
            diff_max_size_bytes_per_side: 64,
            diff_max_line_count: u64::MAX,
            image_max_size_bytes: 96,
            image_max_pixels: u64::MAX,
            pdf_max_size_bytes: 128,
        };
        let mut runtime = test_runtime();
        Arc::get_mut(&mut runtime.inner)
            .expect("unshared test runtime")
            .limits = limits.clone();
        let config = agent_config("agent-a", temp.path());
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01";
        let mut image = png.to_vec();
        image.resize(limits.image_max_size_bytes as usize + 1, b'a');
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.resize(limits.pdf_max_size_bytes as usize + 1, b'a');
        let fixtures = [
            (
                "oversized.txt",
                vec![b'a'; limits.monaco_max_size_bytes as usize + 1],
                "monaco_size_limit_exceeded",
            ),
            ("oversized.png", image, "image_limit_exceeded"),
            ("oversized.pdf", pdf, "pdf_size_limit_exceeded"),
        ];

        for (name, bytes, reason) in fixtures {
            let path = temp.path().join(name);
            fs::write(&path, bytes).expect("oversized fixture");
            let snapshot = runtime
                .open_agent_file("agent-a", &config, &path, None)
                .await
                .expect("oversized resource opens as metadata");

            assert_eq!(snapshot.revision, 1, "{name}");
            assert_eq!(
                snapshot.descriptor.unavailable_reason.as_deref(),
                Some(reason),
                "{name}"
            );
            assert!(
                snapshot
                    .descriptor
                    .content_hash
                    .starts_with("bounded-sha256:"),
                "{name}"
            );
            assert!(!snapshot.descriptor.capabilities.preview, "{name}");
            assert!(!snapshot.descriptor.capabilities.changes, "{name}");
            assert!(!snapshot.descriptor.capabilities.draft, "{name}");
            assert!(!snapshot.descriptor.capabilities.stream, "{name}");
            assert_eq!(
                runtime
                    .read_text(
                        &snapshot.resource_id,
                        &snapshot.subscription_id,
                        snapshot.revision,
                        Some(&config),
                    )
                    .await
                    .expect_err("metadata-only revision must reject text reads")
                    .code(),
                reason,
                "{name}"
            );
            assert_eq!(
                runtime
                    .issue_ticket(
                        &snapshot.resource_id,
                        &snapshot.subscription_id,
                        snapshot.revision,
                        Some(&config),
                        &format!("oversized-{name}"),
                    )
                    .await
                    .expect_err("metadata-only revision must reject tickets")
                    .code(),
                reason,
                "{name}"
            );
        }

        let text_path = temp.path().join("oversized.txt");
        let text_resource = file_resource_id(
            std::fs::canonicalize(&text_path)
                .expect("canonical oversized text")
                .to_string_lossy()
                .as_ref(),
        );
        let mut events = runtime.subscribe_events();
        runtime.schedule_refresh(text_resource.clone());
        sleep(Duration::from_millis(300)).await;
        assert!(
            events.try_recv().is_err(),
            "unchanged bounded fingerprint emitted"
        );

        fs::write(
            &text_path,
            vec![b'b'; limits.monaco_max_size_bytes as usize + 1],
        )
        .expect("same-size oversized rewrite");
        runtime.schedule_refresh(text_resource.clone());
        let changed = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("oversized revision timeout")
            .expect("oversized revision event");
        assert_eq!(changed.revision, 2);
        assert_eq!(
            changed.descriptor.unavailable_reason.as_deref(),
            Some("monaco_size_limit_exceeded")
        );

        fs::write(&text_path, b"recovered\n").expect("recover within renderer limit");
        runtime.schedule_refresh(text_resource);
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("recovery timeout")
            .expect("recovery event");
        assert_eq!(recovered.revision, 3);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
        assert!(recovered.descriptor.content_hash.starts_with("sha256:"));
        assert!(recovered.descriptor.capabilities.preview);
    }

    #[tokio::test]
    async fn ticket_is_exact_repeatable_range_scoped_and_expires() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.pdf");
        let second_path = temp.path().join("second.pdf");
        fs::write(&first_path, b"%PDF-1.7 first payload").expect("first fixture");
        fs::write(&second_path, b"%PDF-1.7 second payload").expect("second fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime =
            FileResourceRuntime::with_timing(Duration::from_millis(50), Duration::from_millis(250));
        let first = runtime
            .open_agent_file("agent-a", &config, &first_path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &second_path, None)
            .await
            .expect("second open");

        let mismatched = runtime
            .issue_ticket(
                &second.resource_id,
                &first.subscription_id,
                second.revision,
                Some(&config),
                "renderer-lease-a",
            )
            .await
            .expect_err("subscription cannot issue for another resource");
        assert_eq!(mismatched.code(), "unauthorized_resource");

        let ticket = runtime
            .issue_ticket_for_webview(
                &first.resource_id,
                &first.subscription_id,
                first.revision,
                Some(&config),
                "renderer-lease-a",
                Some("main"),
            )
            .await
            .expect("ticket");
        assert_eq!(ticket.renderer_lease_id, "renderer-lease-a");
        assert!(ticket.url.starts_with("wardian-resource://"));

        let reused_lease = runtime
            .issue_ticket_for_webview(
                &second.resource_id,
                &second.subscription_id,
                second.revision,
                Some(&config),
                "renderer-lease-a",
                Some("main"),
            )
            .await
            .expect_err("one renderer lease cannot cross subscriptions");
        assert_eq!(reused_lease.code(), "unauthorized_ticket");
        assert_eq!(runtime.ticket_count().await, 1);

        let wrong_webview = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("secondary"))
            .await
            .expect_err("ticket must remain renderer-webview scoped");
        assert_eq!(wrong_webview.code(), "unauthorized_ticket");

        let first_range = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect("first range");
        let repeated_range = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect("repeated range");
        assert_eq!(first_range.bytes, b"%PDF");
        assert_eq!(repeated_range.bytes, first_range.bytes);
        assert_eq!(first_range.mime_type, "application/pdf");

        timeout(Duration::from_secs(2), async {
            loop {
                if runtime.ticket_count().await == 0
                    && runtime.renderer_lease_count().await == 0
                    && runtime.ticket_snapshot_bytes_in_use() == 0
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("abandoned ticket state must be reclaimed at its deadline");
        let expired = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect_err("expired ticket must fail");
        assert_eq!(expired.code(), "invalid_ticket");
    }

    #[tokio::test]
    async fn ticket_serves_its_immutable_revision_after_source_changes_and_is_revoked_on_close() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("lease.pdf");
        fs::write(&path, b"%PDF-1.7 lease payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let ticket = runtime
            .issue_ticket_for_webview(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("ticket");

        fs::write(&path, b"%PDF-1.7 other payload").expect("mutate");
        assert_eq!(
            runtime
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=9-13"), Some("main"))
                .await
                .expect("ticket retains the issued revision")
                .bytes,
            b"lease"
        );
        assert!(runtime
            .verify_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=9-13"), Some("main"),)
            .await
            .expect("HEAD validates immutable snapshot")
            .bytes
            .is_empty());
        fs::remove_file(&path).expect("remove source after issuance");
        assert_eq!(
            runtime
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=9-13"), Some("main"))
                .await
                .expect("range never rereads the removed source")
                .bytes,
            b"lease"
        );

        runtime
            .close(&subscription.subscription_id)
            .await
            .expect("close subscription");
        assert_eq!(
            runtime
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
                .await
                .expect_err("closed subscription must revoke its lease")
                .code(),
            "invalid_ticket"
        );
    }

    #[tokio::test]
    async fn ticket_snapshot_disk_budget_is_bounded_and_released_with_the_lease() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.pdf");
        let second_path = temp.path().join("second.pdf");
        let first_bytes = b"%PDF-1.7 first payload";
        let second_bytes = b"%PDF-1.7 second payload";
        fs::write(&first_path, first_bytes).expect("first fixture");
        fs::write(&second_path, second_bytes).expect("second fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            8,
            MIN_TICKET_SNAPSHOT_RESERVATION_BYTES,
        );
        let first = runtime
            .open_agent_file("agent-a", &config, &first_path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &second_path, None)
            .await
            .expect("second open");
        runtime
            .issue_ticket(
                &first.resource_id,
                &first.subscription_id,
                first.revision,
                Some(&config),
                "first-lease",
            )
            .await
            .expect("first ticket");
        assert_eq!(
            runtime.ticket_snapshot_bytes_in_use(),
            MIN_TICKET_SNAPSHOT_RESERVATION_BYTES
        );

        assert_eq!(
            runtime
                .issue_ticket(
                    &second.resource_id,
                    &second.subscription_id,
                    second.revision,
                    Some(&config),
                    "second-lease",
                )
                .await
                .expect_err("snapshot budget must reject another file")
                .code(),
            "ticket_capacity_exceeded"
        );
        runtime
            .close_renderer_lease(
                &first.resource_id,
                &first.subscription_id,
                "first-lease",
                None,
            )
            .await
            .expect("release first lease");
        assert_eq!(runtime.ticket_snapshot_bytes_in_use(), 0);
        runtime
            .issue_ticket(
                &second.resource_id,
                &second.subscription_id,
                second.revision,
                Some(&config),
                "second-lease",
            )
            .await
            .expect("released budget admits another snapshot");
    }

    #[tokio::test]
    async fn picker_grants_are_exact_path_deduplicated_and_lru_bounded() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.txt");
        let second_path = temp.path().join("second.txt");
        let third_path = temp.path().join("third.txt");
        for path in [&first_path, &second_path, &third_path] {
            fs::write(path, path.to_string_lossy().as_bytes()).expect("fixture");
        }
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            2,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let first = runtime
            .record_user_file(&first_path)
            .await
            .expect("first grant");
        let duplicate = runtime
            .record_user_file(&first_path)
            .await
            .expect("duplicate grant");
        assert_eq!(duplicate.capability_id, first.capability_id);
        assert_eq!(runtime.user_grant_count().await, 1);

        let second = runtime
            .record_user_file(&second_path)
            .await
            .expect("second grant");
        let active_first = runtime
            .open_user_file(&first.capability_id, &first_path, None)
            .await
            .expect("activate first grant");
        let third = runtime
            .record_user_file(&third_path)
            .await
            .expect("third grant evicts inactive LRU");
        assert_eq!(runtime.user_grant_count().await, 2);
        assert!(
            runtime
                .open_user_file(&second.capability_id, &second_path, None)
                .await
                .is_err(),
            "evicted grant must be revoked"
        );
        let active_third = runtime
            .open_user_file(&third.capability_id, &third_path, None)
            .await
            .expect("new grant remains available");
        runtime
            .snapshot(&active_first.resource_id)
            .await
            .expect("active grant is never evicted");
        assert_eq!(
            runtime
                .record_user_file(&second_path)
                .await
                .expect_err("all-active grant set must reject growth")
                .code(),
            "grant_limit_reached"
        );

        runtime
            .close(&active_first.subscription_id)
            .await
            .expect("close first grant");
        runtime
            .record_user_file(&second_path)
            .await
            .expect("closed LRU slot can be reused");
        assert_eq!(runtime.user_grant_count().await, 2);
        runtime
            .snapshot(&active_third.resource_id)
            .await
            .expect("remaining active grant is retained");
    }

    #[tokio::test]
    async fn active_picker_subscription_cannot_be_evicted_after_membership_interleaving() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.txt");
        let second_path = temp.path().join("second.txt");
        fs::write(&first_path, "first\n").expect("first fixture");
        fs::write(&second_path, "second\n").expect("second fixture");
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            1,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let first = runtime
            .record_user_file(&first_path)
            .await
            .expect("first grant");
        let hook = GrantEvictionBeforeLockHook {
            reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.grant_eviction_before_lock_hook.lock().await = Some(hook.clone());

        let competing_selection = {
            let runtime = runtime.clone();
            let second_path = second_path.clone();
            tokio::spawn(async move { runtime.record_user_file(&second_path).await })
        };
        hook.reached.wait().await;
        let active = runtime
            .open_user_file(&first.capability_id, &first_path, None)
            .await
            .expect("open first grant during eviction window");
        {
            let grants = runtime.inner.user_file_grants.lock().await;
            let grant = grants
                .get(&first.capability_id)
                .expect("active first capability");
            assert_eq!(grant.in_flight_uses, 0);
            assert_eq!(grant.active_subscriptions, 1);
        }
        hook.resume.wait().await;
        let selection_error = competing_selection
            .await
            .expect("competing selection task")
            .expect_err("live subscription must make the only grant ineligible for eviction");
        assert_eq!(selection_error.code(), "grant_limit_reached");
        assert!(
            runtime
                .inner
                .user_file_grants
                .lock()
                .await
                .contains_key(&first.capability_id),
            "authoritative activity must retain the first capability"
        );

        *runtime.inner.grant_eviction_before_lock_hook.lock().await = None;
        runtime
            .close(&active.subscription_id)
            .await
            .expect("close active subscription");
        runtime
            .record_user_file(&second_path)
            .await
            .expect("closed grant becomes evictable");
    }

    #[tokio::test]
    async fn renderer_lease_can_be_released_without_closing_shared_subscription() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("release.pdf");
        fs::write(&path, b"%PDF-1.7 release payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let snapshot = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        runtime
            .issue_ticket(
                &snapshot.resource_id,
                &snapshot.subscription_id,
                snapshot.revision,
                Some(&config),
                "renderer-release",
            )
            .await
            .expect("ticket");
        runtime
            .close_renderer_lease(
                &snapshot.resource_id,
                &snapshot.subscription_id,
                "renderer-release",
                None,
            )
            .await
            .expect("release");
        runtime
            .close_renderer_lease(
                &snapshot.resource_id,
                &snapshot.subscription_id,
                "renderer-release",
                None,
            )
            .await
            .expect("idempotent release");

        assert_eq!(runtime.ticket_count().await, 0);
        assert_eq!(runtime.ticket_snapshot_bytes_in_use(), 0);
        assert!(runtime.inner.renderer_leases.lock().await.is_empty());
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(
            runtime
                .snapshot(&snapshot.resource_id)
                .await
                .expect("subscription remains open")
                .subscription_id,
            snapshot.subscription_id
        );
    }

    #[tokio::test]
    async fn reissuing_renderer_lease_purges_superseded_tickets() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("reissue.pdf");
        fs::write(&path, b"%PDF-1.7 reissue payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let first = runtime
            .issue_ticket(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "renderer-reissue",
            )
            .await
            .expect("first ticket");
        let second = runtime
            .issue_ticket(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "renderer-reissue",
            )
            .await
            .expect("replacement ticket");

        assert_eq!(runtime.ticket_count().await, 1);
        assert_eq!(
            runtime
                .read_ticket_range(&first.ticket_id, Some("bytes=0-3"))
                .await
                .expect_err("superseded ticket must be purged")
                .code(),
            "invalid_ticket"
        );
        assert_eq!(
            runtime
                .read_ticket_range(&second.ticket_id, Some("bytes=0-3"))
                .await
                .expect("replacement ticket remains active")
                .bytes,
            b"%PDF"
        );

        runtime
            .close_renderer_lease(
                &subscription.resource_id,
                &subscription.subscription_id,
                "renderer-reissue",
                None,
            )
            .await
            .expect("close replacement lease");
        runtime
            .close_renderer_lease(
                &subscription.resource_id,
                &subscription.subscription_id,
                "renderer-reissue",
                None,
            )
            .await
            .expect("idempotent repeated close");
        assert_eq!(runtime.ticket_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_same_lease_publication_keeps_the_newer_ticket() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("concurrent-reissue.pdf");
        fs::write(&path, b"%PDF-1.7 concurrent reissue payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let hook = TicketPublicationHook {
            pause_once: Arc::new(AtomicBool::new(true)),
            lease_published: Arc::new(tokio::sync::Barrier::new(2)),
            resume_publication: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.ticket_publication_hook.lock().await = Some(hook.clone());

        let first_runtime = runtime.clone();
        let first_config = config.clone();
        let first_resource_id = subscription.resource_id.clone();
        let first_subscription_id = subscription.subscription_id.clone();
        let revision = subscription.revision;
        let first_issue = tokio::spawn(async move {
            first_runtime
                .issue_ticket(
                    &first_resource_id,
                    &first_subscription_id,
                    revision,
                    Some(&first_config),
                    "renderer-concurrent-reissue",
                )
                .await
        });

        hook.lease_published.wait().await;
        let second_runtime = runtime.clone();
        let second_config = config.clone();
        let second_resource_id = subscription.resource_id.clone();
        let second_subscription_id = subscription.subscription_id.clone();
        let second_issue = tokio::spawn(async move {
            second_runtime
                .issue_ticket(
                    &second_resource_id,
                    &second_subscription_id,
                    revision,
                    Some(&second_config),
                    "renderer-concurrent-reissue",
                )
                .await
        });

        sleep(Duration::from_millis(75)).await;
        assert!(
            !second_issue.is_finished(),
            "same-lease publication must serialize behind the in-flight issue"
        );
        hook.resume_publication.wait().await;

        let first = first_issue
            .await
            .expect("first issuance task")
            .expect("first ticket");
        let second = second_issue
            .await
            .expect("second issuance task")
            .expect("replacement ticket");
        assert_eq!(runtime.ticket_count().await, 1);
        assert_eq!(runtime.renderer_lease_count().await, 1);
        assert_eq!(
            runtime
                .read_ticket_range(&first.ticket_id, Some("bytes=0-3"))
                .await
                .expect_err("older concurrent ticket must be purged")
                .code(),
            "invalid_ticket"
        );
        assert_eq!(
            runtime
                .read_ticket_range(&second.ticket_id, Some("bytes=0-3"))
                .await
                .expect("newer concurrent ticket remains active")
                .bytes,
            b"%PDF"
        );
        *runtime.inner.ticket_publication_hook.lock().await = None;
    }

    #[tokio::test]
    async fn reissuing_after_subscription_close_rolls_back_ticket_and_renderer_lease() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("publication-race.pdf");
        fs::write(&path, b"%PDF-1.7 publication race").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        runtime
            .issue_ticket_for_webview(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("initial ticket");
        assert_eq!(runtime.ticket_count().await, 1);
        let hook = IssueTicketAfterValidationHook {
            validation_reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume_publication: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime
            .inner
            .issue_ticket_after_validation_hook
            .lock()
            .await = Some(hook.clone());

        let issuing_runtime = runtime.clone();
        let issuing_config = config.clone();
        let resource_id = subscription.resource_id.clone();
        let subscription_id = subscription.subscription_id.clone();
        let issuance = tokio::spawn(async move {
            issuing_runtime
                .issue_ticket_for_webview(
                    &resource_id,
                    &subscription_id,
                    subscription.revision,
                    Some(&issuing_config),
                    "lease-a",
                    Some("main"),
                )
                .await
        });

        hook.validation_reached.wait().await;
        runtime
            .close(&subscription.subscription_id)
            .await
            .expect("close completes while issuance is paused");
        hook.resume_publication.wait().await;

        let issue_error = issuance
            .await
            .expect("issuance task")
            .expect_err("closed subscription cannot publish a ticket");
        assert_eq!(issue_error.code(), "invalid_ticket");
        assert!(runtime.inner.read_tickets.lock().await.is_empty());
        assert!(runtime.inner.renderer_leases.lock().await.is_empty());

        *runtime
            .inner
            .issue_ticket_after_validation_hook
            .lock()
            .await = None;
        let reopened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("reopen");
        runtime
            .issue_ticket_for_webview(
                &reopened.resource_id,
                &reopened.subscription_id,
                reopened.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("new subscription reuses renderer lease immediately");
    }

    #[tokio::test]
    async fn application_cleanup_closes_watchers_grants_and_tickets() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("cleanup.pdf");
        fs::write(&path, b"%PDF-1.7 cleanup payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        runtime
            .issue_ticket(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "renderer-lease-a",
            )
            .await
            .expect("ticket");
        let save_target = temp.path().join("cleanup-copy.txt");
        runtime
            .record_save_target(&save_target)
            .await
            .expect("save target grant");

        runtime.close_all().await;

        assert_eq!(runtime.watcher_count().await, 0);
        assert_eq!(runtime.ticket_count().await, 0);
        assert_eq!(runtime.user_grant_count().await, 0);
        assert!(runtime.inner.save_target_grants.lock().await.is_empty());
        assert!(runtime.inner.renderer_leases.lock().await.is_empty());
    }

    fn create_directory_link(target: &Path, link: &Path) {
        wardian_core::library::create_directory_link(target, link).expect("directory link");
    }

    fn remove_directory_link(link: &Path) {
        wardian_core::library::remove_existing_deployment(link).expect("remove directory link");
    }

    #[cfg(unix)]
    fn replace_path_identity(replacement: &Path, target: &Path) {
        fs::rename(replacement, target).expect("atomic replacement");
    }

    #[cfg(windows)]
    fn replace_path_identity(replacement: &Path, target: &Path) {
        let prior = target.with_extension("prior");
        fs::rename(target, &prior).expect("move prior identity aside");
        fs::rename(replacement, target).expect("move replacement into target");
        fs::remove_file(prior).expect("remove prior identity");
    }
}
