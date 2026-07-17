//! Backend-owned file subscriptions, stable revisions, and bounded read leases.

use notify::Watcher as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Emitter as _;
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
    limits: FileResourceLimits,
    stability_delay: Duration,
    ticket_ttl: Duration,
    events: broadcast::Sender<FileResourceEventV1>,
    app_handle: RwLock<Option<tauri::AppHandle>>,
    #[cfg(test)]
    issue_ticket_after_validation_hook: Mutex<Option<IssueTicketAfterValidationHook>>,
}

#[cfg(test)]
#[derive(Clone)]
struct IssueTicketAfterValidationHook {
    validation_reached: Arc<tokio::sync::Barrier>,
    resume_publication: Arc<tokio::sync::Barrier>,
}

struct FileResourceEntry {
    _watcher: notify::RecommendedWatcher,
    authorized: AuthorizedPath,
    revision_token: FileRevisionToken,
    descriptor: FileContentDescriptorV1,
    revision: u64,
    incarnation_id: Uuid,
    subscribers: HashMap<String, FileAccessClaim>,
    debounce_generation: u64,
}

#[derive(Clone)]
enum FileAccessClaim {
    Agent { agent_id: String },
    User { capability_id: String },
}

#[derive(Clone)]
struct UserFileGrant {
    canonical_path: String,
    authorized: AuthorizedPath,
}

#[derive(Clone)]
struct FileReadTicket {
    issuance_id: Uuid,
    webview_label: Option<String>,
    renderer_lease: RendererLeaseKey,
    subscription_id: String,
    resource_id: String,
    authorized: AuthorizedPath,
    revision_token: FileRevisionToken,
    size_bytes: u64,
    mime_type: String,
    expires_at: Instant,
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
                limits: FileResourceLimits::default(),
                stability_delay,
                ticket_ttl,
                events,
                app_handle: RwLock::new(None),
                #[cfg(test)]
                issue_ticket_after_validation_hook: Mutex::new(None),
            }),
        }
    }

    pub fn attach_app_handle(&self, app_handle: tauri::AppHandle) {
        match self.inner.app_handle.write() {
            Ok(mut current) => *current = Some(app_handle),
            Err(poisoned) => *poisoned.into_inner() = Some(app_handle),
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
        let parent = selected_path.parent().ok_or_else(|| {
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
        let authorized = AuthorizedRootService::from_agent_config(&config)?
            .authorize_existing_file(selected_path)?;
        let snapshot = verified_snapshot(authorized.clone(), self.inner.limits.clone()).await?;
        let canonical_path = snapshot.descriptor().canonical_path.clone();
        let capability_id = Uuid::new_v4().to_string();
        self.inner.user_file_grants.lock().await.insert(
            capability_id.clone(),
            UserFileGrant {
                canonical_path: canonical_path.clone(),
                authorized,
            },
        );
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
        let grant = self
            .inner
            .user_file_grants
            .lock()
            .await
            .get(capability_id)
            .cloned()
            .ok_or_else(|| error("unauthorized_path", "user file capability is unavailable"))?;
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
        if requested != grant.canonical_path {
            return Err(error(
                "unauthorized_path",
                "user file capability grants only the selected canonical file",
            ));
        }
        self.open_authorized(
            grant.authorized,
            FileAccessClaim::User {
                capability_id: capability_id.to_string(),
            },
        )
        .await
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
                entry.subscribers.insert(subscription_id.clone(), claim);
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
                    .insert(subscription_id, resource_id);
                return Ok(result);
            }
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
        let result = {
            let mut entries = self.inner.entries.lock().await;
            if let Some(entry) = entries.get_mut(&resource_id) {
                entry.subscribers.insert(subscription_id.clone(), claim);
                FileResourceSnapshotV1 {
                    resource_id: resource_id.clone(),
                    subscription_id: subscription_id.clone(),
                    revision: entry.revision,
                    descriptor: entry.descriptor.clone(),
                }
            } else {
                let mut subscribers = HashMap::new();
                subscribers.insert(subscription_id.clone(), claim);
                entries.insert(
                    resource_id.clone(),
                    FileResourceEntry {
                        _watcher: watcher,
                        authorized,
                        revision_token,
                        descriptor: descriptor.clone(),
                        revision: 1,
                        incarnation_id,
                        subscribers,
                        debounce_generation: 0,
                    },
                );
                FileResourceSnapshotV1 {
                    resource_id: resource_id.clone(),
                    subscription_id: subscription_id.clone(),
                    revision: 1,
                    descriptor,
                }
            }
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
        let authorized = {
            let entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get(resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id || entry.debounce_generation != generation {
                return;
            }
            entry.authorized.clone()
        };
        let Ok(snapshot) = verified_snapshot(authorized.clone(), self.inner.limits.clone()).await
        else {
            return;
        };
        let (descriptor, revision_token) = snapshot.into_parts();
        let event = {
            let mut entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get_mut(resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id || entry.debounce_generation != generation {
                return;
            }
            entry.authorized = authorized;
            entry.revision_token = revision_token;
            if entry.descriptor.content_hash == descriptor.content_hash {
                entry.descriptor = descriptor;
                return;
            }
            entry.revision = entry.revision.saturating_add(1);
            entry.descriptor = descriptor.clone();
            FileResourceEventV1 {
                schema: 1,
                resource_id: resource_id.to_string(),
                revision: entry.revision,
                descriptor,
            }
        };
        self.emit(event);
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
        let mut entries = self.inner.entries.lock().await;
        if let Some(entry) = entries.get_mut(&resource_id) {
            entry.subscribers.remove(subscription_id);
            if entry.subscribers.is_empty() {
                entries.remove(&resource_id);
            }
        }
        drop(entries);
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
            .next()
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
        let claim = entries
            .get(resource_id)
            .and_then(|entry| entry.subscribers.get(subscription_id))
            .ok_or_else(|| {
                error(
                    "unauthorized_resource",
                    "subscription does not grant the requested resource",
                )
            })?;
        Ok(match claim {
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
        let (claim, authorized, expected, revision_token) = {
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
            let claim = entry
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
                claim,
                entry.authorized.clone(),
                entry.descriptor.clone(),
                entry.revision_token.clone(),
            )
        };

        match claim {
            FileAccessClaim::Agent { agent_id } => {
                let config = current_agent_config.ok_or_else(|| {
                    error(
                        "unauthorized_path",
                        "current agent authorization is unavailable",
                    )
                })?;
                if config.session_id != agent_id {
                    return Err(error(
                        "unauthorized_path",
                        "current agent authorization does not match the subscription",
                    ));
                }
                let current = AuthorizedRootService::from_agent_config(config)?
                    .authorize_existing_file(Path::new(&expected.canonical_path))?;
                if current.canonical_path != authorized.canonical_path {
                    return Err(error(
                        "unauthorized_path",
                        "current agent authorization resolves to another file",
                    ));
                }
            }
            FileAccessClaim::User { capability_id } => {
                let grant = self
                    .inner
                    .user_file_grants
                    .lock()
                    .await
                    .get(&capability_id)
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
                if grant.authorized.canonical_path != authorized.canonical_path {
                    return Err(error(
                        "unauthorized_path",
                        "user file capability resolves to another file",
                    ));
                }
            }
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
        let ticket_id = Uuid::new_v4().to_string();
        let issuance_id = Uuid::new_v4();
        let expires_at = Instant::now() + self.inner.ticket_ttl;
        let renderer_lease = RendererLeaseKey {
            webview_label: webview_label.map(str::to_string),
            renderer_lease_id: renderer_lease_id.to_string(),
        };
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
            renderer_lease,
            subscription_id: subscription_id.to_string(),
            resource_id: resource_id.to_string(),
            authorized,
            revision_token,
            size_bytes: descriptor.size_bytes,
            mime_type: descriptor.mime_type,
            expires_at,
        };
        self.inner
            .read_tickets
            .lock()
            .await
            .insert(ticket_id.clone(), ticket.clone());
        if let Err(error) = self.ensure_ticket_lease_active(&ticket).await {
            self.rollback_ticket_publication(&ticket_id, &ticket).await;
            return Err(error);
        }
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
        let revision_token = ticket.revision_token.clone();
        let authorized = ticket.authorized.clone();
        let maximum_length_bytes = ticket.size_bytes;
        let bytes = tauri::async_runtime::spawn_blocking(move || {
            authorized.read_verified_range(&revision_token, range.0..=range.1, maximum_length_bytes)
        })
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
        let revision_token = ticket.revision_token.clone();
        let authorized = ticket.authorized.clone();
        tauri::async_runtime::spawn_blocking(move || authorized.verify_revision(&revision_token))
            .await
            .map_err(join_error)??;
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
            self.inner.read_tickets.lock().await.remove(ticket_id);
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
    pub async fn user_grant_count(&self) -> usize {
        self.inner.user_file_grants.lock().await.len()
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

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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

        sleep(Duration::from_millis(300)).await;
        let expired = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect_err("expired ticket must fail");
        assert_eq!(expired.code(), "expired_ticket");
    }

    #[tokio::test]
    async fn ticket_rejects_stale_content_and_is_revoked_with_its_subscription() {
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
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
                .await
                .expect_err("stale ticket must fail")
                .code(),
            "stale_revision"
        );

        fs::write(&path, b"%PDF-1.7 lease payload").expect("restore");
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
    async fn close_before_ticket_publication_rolls_back_ticket_and_renderer_lease() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("publication-race.pdf");
        fs::write(&path, b"%PDF-1.7 publication race").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
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
}
