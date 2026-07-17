//! Backend-owned file subscriptions, stable revisions, and bounded read leases.

use notify::Watcher as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter as _, Manager as _};
use tokio::sync::{broadcast, Mutex};
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
const MAX_TICKET_SNAPSHOT_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_TICKET_SNAPSHOT_RESERVATION_BYTES: u64 = 4 * 1024 * 1024;

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
    user_file_grants: Mutex<HashMap<String, UserFileGrant>>,
    read_tickets: Mutex<HashMap<String, FileReadTicket>>,
    renderer_leases: Mutex<HashMap<RendererLeaseKey, RendererLease>>,
    ticket_publication: Mutex<()>,
    limits: FileResourceLimits,
    stability_delay: Duration,
    ticket_ttl: Duration,
    max_user_file_grants: usize,
    ticket_snapshot_usage: Arc<AtomicU64>,
    max_ticket_snapshot_bytes: u64,
    events: broadcast::Sender<FileResourceEventV1>,
    app_handle: RwLock<Option<tauri::AppHandle>>,
    agent_config_resolver: RwLock<CurrentAgentConfigResolver>,
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

struct FileResourceEntry {
    _watcher: notify::RecommendedWatcher,
    revision_token: FileRevisionToken,
    descriptor: FileContentDescriptorV1,
    revision: u64,
    incarnation_id: Uuid,
    subscribers: HashMap<String, FileSubscriptionAccess>,
    debounce_generation: u64,
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
                user_file_grants: Mutex::new(HashMap::new()),
                read_tickets: Mutex::new(HashMap::new()),
                renderer_leases: Mutex::new(HashMap::new()),
                ticket_publication: Mutex::new(()),
                limits: FileResourceLimits::default(),
                stability_delay,
                ticket_ttl,
                max_user_file_grants: DEFAULT_MAX_USER_FILE_GRANTS,
                ticket_snapshot_usage: Arc::new(AtomicU64::new(0)),
                max_ticket_snapshot_bytes: MAX_TICKET_SNAPSHOT_BYTES,
                events,
                app_handle: RwLock::new(None),
                agent_config_resolver: RwLock::new(CurrentAgentConfigResolver::default()),
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
                user_file_grants: Mutex::new(HashMap::new()),
                read_tickets: Mutex::new(HashMap::new()),
                renderer_leases: Mutex::new(HashMap::new()),
                ticket_publication: Mutex::new(()),
                limits: FileResourceLimits::default(),
                stability_delay,
                ticket_ttl,
                max_user_file_grants,
                ticket_snapshot_usage: Arc::new(AtomicU64::new(0)),
                max_ticket_snapshot_bytes,
                events,
                app_handle: RwLock::new(None),
                agent_config_resolver: RwLock::new(CurrentAgentConfigResolver::default()),
                issue_ticket_after_validation_hook: Mutex::new(None),
                ticket_publication_hook: Mutex::new(None),
                forced_refresh_error: Mutex::new(None),
                open_after_entry_miss_hook: Mutex::new(None),
                grant_eviction_before_lock_hook: Mutex::new(None),
                refresh_scan_count: AtomicU64::new(0),
            }),
        }
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
                authorized.clone(),
                FileAccessClaim::User {
                    capability_id: capability_id.to_string(),
                },
            )
            .await;
        self.finish_user_grant_open(capability_id, result.as_ref().ok().map(|_| authorized))
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
        let now = Instant::now();
        let mut grants = self.inner.user_file_grants.lock().await;
        if let Some((capability_id, existing)) = grants
            .iter_mut()
            .find(|(_, grant)| grant.canonical_path == canonical_path)
        {
            existing.authorized = authorized;
            existing.last_used_at = now;
            return Ok(capability_id.clone());
        }

        if grants.len() >= self.inner.max_user_file_grants {
            let evict = grants
                .iter()
                .filter(|(_, grant)| grant.in_flight_uses == 0 && grant.active_subscriptions == 0)
                .min_by_key(|(_, grant)| grant.last_used_at)
                .map(|(capability_id, _)| capability_id.clone());
            let Some(evict) = evict else {
                return Err(error(
                    "grant_limit_reached",
                    "all exact-file grants are active; close a file before selecting another",
                ));
            };
            grants.remove(&evict);
        }

        let capability_id = Uuid::new_v4().to_string();
        grants.insert(
            capability_id.clone(),
            UserFileGrant {
                canonical_path,
                authorized,
                last_used_at: now,
                in_flight_uses: 0,
                active_subscriptions: 0,
            },
        );
        Ok(capability_id)
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

        {
            let mut entries = self.inner.entries.lock().await;
            if let Some(entry) = entries.get_mut(&resource_id) {
                let incarnation_id = entry.incarnation_id;
                entry.subscribers.insert(
                    subscription_id.clone(),
                    FileSubscriptionAccess { claim, authorized },
                );
                let result = FileResourceSnapshotV1 {
                    resource_id: resource_id.clone(),
                    subscription_id: subscription_id.clone(),
                    revision: entry.revision,
                    descriptor: entry.descriptor.clone(),
                };
                drop(entries);
                self.inner
                    .subscription_resources
                    .lock()
                    .await
                    .insert(subscription_id, resource_id.clone());
                self.schedule_refresh_for_incarnation(resource_id, incarnation_id);
                return Ok(result);
            }
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
        let (result, existing_incarnation_id) = {
            let mut entries = self.inner.entries.lock().await;
            if let Some(entry) = entries.get_mut(&resource_id) {
                let existing_incarnation_id = entry.incarnation_id;
                entry.subscribers.insert(
                    subscription_id.clone(),
                    FileSubscriptionAccess { claim, authorized },
                );
                (
                    FileResourceSnapshotV1 {
                        resource_id: resource_id.clone(),
                        subscription_id: subscription_id.clone(),
                        revision: entry.revision,
                        descriptor: entry.descriptor.clone(),
                    },
                    Some(existing_incarnation_id),
                )
            } else {
                let mut subscribers = HashMap::new();
                subscribers.insert(
                    subscription_id.clone(),
                    FileSubscriptionAccess { claim, authorized },
                );
                entries.insert(
                    resource_id.clone(),
                    FileResourceEntry {
                        _watcher: watcher,
                        revision_token,
                        descriptor: descriptor.clone(),
                        revision: 1,
                        incarnation_id,
                        subscribers,
                        debounce_generation: 0,
                    },
                );
                (
                    FileResourceSnapshotV1 {
                        resource_id: resource_id.clone(),
                        subscription_id: subscription_id.clone(),
                        revision: 1,
                        descriptor,
                    },
                    None,
                )
            }
        };
        self.inner
            .subscription_resources
            .lock()
            .await
            .insert(subscription_id.clone(), resource_id.clone());
        if let Some(existing_incarnation_id) = existing_incarnation_id {
            self.schedule_refresh_for_incarnation(resource_id, existing_incarnation_id);
        } else if pending_event.swap(false, Ordering::AcqRel) {
            self.schedule_refresh_for_incarnation(resource_id, incarnation_id);
        }
        Ok(result)
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
            .remove(subscription_id);
        let Some(resource_id) = resource_id else {
            return Ok(());
        };
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
        let (access, expected, revision_token) = {
            let entries = self.inner.entries.lock().await;
            let entry = entries
                .get(resource_id)
                .ok_or_else(|| error("resource_not_found", "file resource is not open"))?;
            if entry.revision != revision {
                return Err(error(
                    "stale_revision",
                    "requested revision is no longer current",
                ));
            }
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
        Ok((authorized, expected, revision_token))
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
        self.inner.entries.lock().await.clear();
        self.inner.subscription_resources.lock().await.clear();
        self.inner.user_file_grants.lock().await.clear();
        self.inner.read_tickets.lock().await.clear();
        self.inner.renderer_leases.lock().await.clear();
        match self.inner.app_handle.write() {
            Ok(mut current) => *current = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
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

        runtime.close_all().await;

        assert_eq!(runtime.watcher_count().await, 0);
        assert_eq!(runtime.ticket_count().await, 0);
        assert_eq!(runtime.user_grant_count().await, 0);
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
