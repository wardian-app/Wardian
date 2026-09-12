#[derive(Clone)]
pub struct FileResourceRuntime {
    inner: Arc<FileResourceRuntimeInner>,
}

struct FileResourceRuntimeInner {
    entries: Mutex<HashMap<String, FileResourceEntry>>,
    subscription_resources: Mutex<HashMap<String, String>>,
    user_file_grants: Arc<Mutex<HashMap<String, UserFileGrant>>>,
    user_file_grant_store_path: RwLock<Option<PathBuf>>,
    user_file_grant_store_io: Mutex<()>,
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
    #[cfg(test)]
    watcher_refresh_enabled: AtomicBool,
    #[cfg(test)]
    debounce_wait_probe: StdMutex<Option<DebounceWaitProbe>>,
}

// Observes the real debounce timer without controlling its completion.
#[cfg(test)]
#[derive(Default)]
struct DebounceWaitProbe {
    armed: Vec<(u64, tokio::time::Instant)>,
    completed: Vec<u64>,
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
    Local,
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

impl FileResourceRuntime {
    #[must_use]
    pub fn with_timing(stability_delay: Duration, ticket_ttl: Duration) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(FileResourceRuntimeInner {
                entries: Mutex::new(HashMap::new()),
                subscription_resources: Mutex::new(HashMap::new()),
                user_file_grants: Arc::new(Mutex::new(HashMap::new())),
                user_file_grant_store_path: RwLock::new(None),
                user_file_grant_store_io: Mutex::new(()),
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
                #[cfg(test)]
                watcher_refresh_enabled: AtomicBool::new(true),
                #[cfg(test)]
                debounce_wait_probe: StdMutex::new(None),
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
                user_file_grant_store_path: RwLock::new(None),
                user_file_grant_store_io: Mutex::new(()),
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
                watcher_refresh_enabled: AtomicBool::new(true),
                debounce_wait_probe: StdMutex::new(None),
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

    #[cfg(test)]
    fn configure_user_file_grant_store_for_test(&self, path: PathBuf) {
        self.configure_user_file_grant_store(path);
    }

    pub fn attach_app_handle(&self, app_handle: tauri::AppHandle) {
        if let Some(path) = default_user_file_grant_store_path() {
            self.configure_user_file_grant_store(path);
        }
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

    fn configure_user_file_grant_store(&self, path: PathBuf) {
        match self.inner.user_file_grant_store_path.write() {
            Ok(mut current) => {
                if current.is_none() {
                    *current = Some(path);
                }
            }
            Err(poisoned) => {
                let mut current = poisoned.into_inner();
                if current.is_none() {
                    *current = Some(path);
                }
            }
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

    /// Opens an existing local file for the Workbench without coupling the
    /// subscription lifetime to an agent process or native-picker capability.
    /// The exact path binding and retained file handle are still revalidated
    /// for every read, refresh, ticket, and save.
    pub async fn open_local_file(
        &self,
        path: &Path,
        app_handle: Option<tauri::AppHandle>,
    ) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
        if let Some(app_handle) = app_handle {
            self.attach_app_handle(app_handle);
        }
        let authorized = authorize_user_file_path(path)?;
        self.open_authorized(authorized, FileAccessClaim::Local)
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
        self.persist_durable_user_file_grant(&canonical_path)
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
        self.persist_durable_user_file_grant(&canonical_path)
            .await?;
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

    fn user_file_grant_store_path(&self) -> Option<PathBuf> {
        self.inner
            .user_file_grant_store_path
            .read()
            .map(|path| path.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    async fn persist_durable_user_file_grant(
        &self,
        canonical_path: &str,
    ) -> Result<(), FileResourceErrorV1> {
        let Some(store_path) = self.user_file_grant_store_path() else {
            return Ok(());
        };
        let _io = self.inner.user_file_grant_store_io.lock().await;
        let canonical_path = canonical_path.to_string();
        let max_grants = self.inner.max_user_file_grants;
        tauri::async_runtime::spawn_blocking(move || {
            upsert_durable_user_file_grant(&store_path, &canonical_path, max_grants)
        })
        .await
        .map_err(join_error)?
    }

    async fn durable_user_file_grant_matches(
        &self,
        canonical_path: &str,
    ) -> Result<bool, FileResourceErrorV1> {
        let Some(store_path) = self.user_file_grant_store_path() else {
            return Ok(false);
        };
        let _io = self.inner.user_file_grant_store_io.lock().await;
        let canonical_path = canonical_path.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            durable_user_file_grant_matches(&store_path, &canonical_path)
        })
        .await
        .map_err(join_error)?
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
        let capability_id = match capability_id {
            Some(capability_id) => capability_id,
            None if self.durable_user_file_grant_matches(requested).await? => {
                self.record_user_file(path).await?.capability_id
            }
            None => return Ok(None),
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
                    // A controlled timer test disables callbacks before opening its file.
                    // Check before pending_event so the post-open path stays disabled too.
                    #[cfg(test)]
                    if weak
                        .upgrade()
                        .is_some_and(|inner| !inner.watcher_refresh_enabled.load(Ordering::Acquire))
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
            runtime
                .refresh_after_stability(resource_id, incarnation_id)
                .await;
        });
    }

    // Keep generation registration and the timer in the same future. Production
    // runs it on Tauri; the timer regression drives it on its own paused clock.
    async fn refresh_after_stability(&self, resource_id: String, incarnation_id: Uuid) {
        let generation = {
            let mut entries = self.inner.entries.lock().await;
            let Some(entry) = entries.get_mut(&resource_id) else {
                return;
            };
            if entry.incarnation_id != incarnation_id {
                return;
            }
            entry.debounce_generation = entry.debounce_generation.saturating_add(1);
            entry.debounce_generation
        };
        let stability_wait = tokio::time::sleep(self.inner.stability_delay);
        #[cfg(test)]
        if let Some(probe) = self
            .inner
            .debounce_wait_probe
            .lock()
            .expect("debounce wait probe")
            .as_mut()
        {
            probe.armed.push((generation, stability_wait.deadline()));
        }
        stability_wait.await;
        #[cfg(test)]
        if let Some(probe) = self
            .inner
            .debounce_wait_probe
            .lock()
            .expect("debounce wait probe")
            .as_mut()
        {
            probe.completed.push(generation);
        }
        self.refresh_if_stable(&resource_id, incarnation_id, generation)
            .await;
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
                            FileAccessClaim::Agent { .. } | FileAccessClaim::Local => None,
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
            FileAccessClaim::Local => Ok(rebound),
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
            FileAccessClaim::User { .. } | FileAccessClaim::Local => None,
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
            FileAccessClaim::Local => {
                let current = access.authorized.reauthorize_same_target()?;
                if current.canonical_path != Path::new(&expected.canonical_path) {
                    return Err(error(
                        "unavailable_path",
                        "local file subscription resolves to another file",
                    ));
                }
                current
            }
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
                FileAccessClaim::Agent { .. } | FileAccessClaim::Local => None,
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
                        FileAccessClaim::Agent { .. } | FileAccessClaim::Local => None,
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
                        FileAccessClaim::Agent { .. } | FileAccessClaim::Local => None,
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
    let canonical_path = windows_file_resource_identity(canonical_path);
    #[cfg(not(windows))]
    let canonical_path = canonical_path.to_string();
    format!("file:{canonical_path}")
}

#[cfg(windows)]
fn windows_file_resource_identity(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    if slashed
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/UNC/"))
    {
        return format!("//{}", &slashed[8..]);
    }
    if slashed
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/"))
        && slashed
            .get(4..7)
            .is_some_and(|drive| drive.as_bytes().get(1) == Some(&b':'))
    {
        return slashed[4..].to_string();
    }
    slashed
}
