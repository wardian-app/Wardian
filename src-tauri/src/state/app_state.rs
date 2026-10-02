use crate::remote::models::RemoteAgentSummary;
use crate::state::active_agent::ActiveAgent;
use crate::state::artifact_runtime::ArtifactRuntime;
use crate::state::browser_session::BrowserSessionBroker;
use crate::state::change_snapshot_runtime::ChangeSnapshotRuntime;
use crate::state::conversation_archive::ConversationArchiveState;
use crate::state::file_resources::FileResourceRuntime;
use crate::state::interactions::InteractionState;
use crate::state::terminal_session::TerminalSessionBroker;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock, Weak};
use tokio::sync::Mutex;

pub struct LibraryWatchRegistration {
    pub watcher: notify::RecommendedWatcher,
    pub ref_count: usize,
    pub generation: u64,
    pub watched_paths: Vec<PathBuf>,
}

pub struct ExplorerWatchRegistration {
    pub watcher: notify::RecommendedWatcher,
    pub ref_count: usize,
}

#[derive(Default)]
struct StatusRevisionSession {
    high_water: u64,
    by_arc: HashMap<usize, StatusArcRevision>,
}

struct StatusArcRevision {
    owner: Weak<std::sync::Mutex<String>>,
    intent_revision: u64,
    intent_status: String,
    committed_revision: u64,
}

impl StatusRevisionSession {
    fn current(&self, status: &Arc<std::sync::Mutex<String>>) -> Option<&StatusArcRevision> {
        self.by_arc
            .get(&(Arc::as_ptr(status) as usize))
            .filter(|entry| {
                entry
                    .owner
                    .upgrade()
                    .is_some_and(|owner| Arc::ptr_eq(&owner, status))
            })
    }

    fn next(&mut self) -> u64 {
        self.by_arc
            .retain(|_, entry| entry.owner.strong_count() > 0);
        self.high_water += 1;
        self.high_water
    }
}

pub struct AppState {
    // Serializes workbench load/save/reset commands before the core's per-home
    // disk CAS lock, keeping the async command boundary ordered without a
    // synchronous mutex held across an await.
    pub workbench_io_lock: Mutex<()>,
    // Serializes queue read-modify-write mutations shared by the desktop and
    // remote Inbox surfaces.
    pub queue_io_lock: Mutex<()>,
    // Snapshot returned by the desktop queue load, used to merge a later
    // desktop save with remote mutations that happened in between.
    pub queue_loaded_snapshot: Mutex<Option<Vec<serde_json::Value>>>,
    // Map of session_id to ActiveAgent
    pub agents: Mutex<HashMap<String, ActiveAgent>>,
    pub system_metrics: Arc<Mutex<sysinfo::System>>,
    pub agent_order: Mutex<Vec<String>>,
    pub agent_name_reservations: Mutex<HashSet<String>>,
    pub agent_lifecycle_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub delivery_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub status_observation_sequences: std::sync::Mutex<HashMap<String, u64>>,
    status_revisions: std::sync::Mutex<HashMap<String, StatusRevisionSession>>,
    pub automation_scheduler_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub automation_schedules_paused: std::sync::atomic::AtomicBool,
    // Active git repo watchers keyed by workspace path
    pub git_watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
    // Active library watchers keyed by library type, shared by mounted UI consumers
    pub library_watchers: Mutex<HashMap<String, LibraryWatchRegistration>>,
    // Active explorer root watchers keyed by normalized root path
    pub explorer_watchers: Mutex<HashMap<String, ExplorerWatchRegistration>>,
    // Canonical file subscriptions, stable revisions, exact grants, and read leases.
    pub file_resources: FileResourceRuntime,
    // Live acknowledgement rendezvous for durable artifact presentations.
    pub artifact_runtime: Arc<ArtifactRuntime>,
    // Single standalone terminal session for the human user.
    pub user_terminal: Mutex<Option<crate::state::UserTerminalSession>>,
    // Live-only structured ask/reply requests keyed by backend-owned request id.
    pub interactions: Arc<InteractionState>,
    /// Wardian-owned persistent provider-session actors. Provider identities
    /// remain generation-bound diagnostics behind this broker.
    pub native_delivery: Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    /// Orders provider-log policy observations before per-agent archive cursor
    /// commits. It has two lanes: policy transitions and lifecycle boundaries
    /// queue in order, while best-effort captures poll and stand aside for a
    /// registered boundary (see `capture_policy_gate`). A capture snapshots its
    /// agent after taking the gate; a policy transition snapshots the roster
    /// before it. Per-agent archive locks come only after the gate, and no
    /// holder of `state.agents` may wait for it.
    pub conversation_capture_policy_lock: crate::state::capture_policy_gate::CapturePolicyGate,
    pub conversation_archive: ConversationArchiveState,
    /// Agents whose New Session is running, so a repeated request is refused
    /// instead of queueing behind the first one. A std mutex because the
    /// claim's `Drop` cannot await; it is never held across an await.
    pub clears_in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
    // Serializes and coalesces per-turn change snapshots, one slot per workspace.
    pub change_snapshots: ChangeSnapshotRuntime,
    // Live-only remote-control authentication and ticket records.
    pub remote_runtime: Mutex<crate::remote::models::RemoteRuntimeState>,
    pub remote_listener: crate::remote::listener::RemoteListener,
    /// Orders persisted settings and startup so an older startup snapshot
    /// cannot reopen a listener after remote access was disabled.
    pub remote_gateway_config_lock: Mutex<()>,
    // Last complete remote roster. The gateway uses this while a provider or
    // telemetry task temporarily owns a live agent snapshot lock.
    pub remote_agent_roster_cache: RwLock<Option<Vec<RemoteAgentSummary>>>,
    // Status observations are updated independently of the global agent map so
    // a remote read can still report live state while that map is busy.
    pub remote_agent_status_cache: RwLock<HashMap<String, (u64, String)>>,
    // Filesystem-heavy automation Inbox reconciliation is refreshed at most
    // once per short interval and served from this cache by the fast remote
    // compatibility endpoint.
    pub remote_inbox_runtime_cache: RwLock<Option<Vec<serde_json::Value>>>,
    pub remote_inbox_runtime_refreshing: std::sync::atomic::AtomicBool,
    pub remote_inbox_runtime_refreshed_at: std::sync::atomic::AtomicI64,
    pub remote_inbox_runtime_refresh_failed: std::sync::atomic::AtomicBool,
    pub remote_inbox_runtime_generation: std::sync::atomic::AtomicU64,
    // Last frontend-reported effective theme. The frontend resolves "system"
    // before updating this so native PTY fallbacks can answer light/dark probes.
    pub terminal_theme: RwLock<String>,
    // Authoritative per-runtime terminal actors. Presentations and feed
    // consumers attach to this broker without owning PTY lifetime or queues.
    pub terminal_sessions: Arc<TerminalSessionBroker>,
    /// Out-of-process browser runtimes backing browser surfaces.
    pub browser_sessions: Arc<BrowserSessionBroker>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the active agents as topology resolver inputs
    /// (uuid + configured workspace folder, empty folder → None).
    pub async fn topology_agent_refs(&self) -> Vec<wardian_core::topology::AgentRef> {
        let agents_map = self.agents.lock().await;
        agents_map
            .iter()
            .map(|(uuid, agent)| {
                let workspace = agent.config.lock().ok().and_then(|c| {
                    let folder = c.folder.trim();
                    if folder.is_empty() {
                        None
                    } else {
                        Some(folder.to_string())
                    }
                });
                wardian_core::topology::AgentRef {
                    uuid: uuid.clone(),
                    workspace,
                }
            })
            .collect()
    }

    pub async fn delivery_lock_for(&self, target_session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.delivery_locks.lock().await;
        locks
            .entry(target_session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Serializes every external writer to one agent PTY. Multi-step provider
    /// interactions retain this guard for their whole transaction so raw
    /// terminal input cannot be inserted between automated steps.
    pub async fn lock_agent_delivery(
        &self,
        target_session_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        self.delivery_lock_for(target_session_id)
            .await
            .lock_owned()
            .await
    }

    /// Returns the gate shared by agent lifecycle transitions and headless
    /// provider runs for one registered agent. Keeping this ownership in
    /// `AppState` prevents a resumed live session from overlapping an in-flight
    /// headless use of the same saved provider conversation.
    pub async fn agent_lifecycle_lock_for(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.agent_lifecycle_locks.lock().await;
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn lock_agent_lifecycle(&self, session_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.agent_lifecycle_lock_for(session_id)
            .await
            .lock_owned()
            .await
    }

    /// Tries to claim an agent's lifecycle gate without waiting. Headless
    /// message delivery uses this to preserve QueueIfBusy behavior when a
    /// lifecycle operation or another headless request already owns the agent.
    pub async fn try_lock_agent_lifecycle(
        &self,
        session_id: &str,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        self.agent_lifecycle_lock_for(session_id)
            .await
            .try_lock_owned()
            .ok()
    }

    pub async fn remove_agent_delivery_state(&self, target_session_id: &str) {
        self.delivery_locks.lock().await.remove(target_session_id);
        if let Ok(mut sequences) = self.status_observation_sequences.lock() {
            sequences.remove(target_session_id);
        }
        if let Ok(mut revisions) = self.status_revisions.lock() {
            revisions.remove(target_session_id);
        }
        self.remote_agent_status_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(target_session_id);
        self.interactions
            .clear_provider_input_state_in_memory(target_session_id)
            .await;
        if let Err(error) = self.native_delivery.dispose_agent(target_session_id).await {
            crate::utils::logging::log_debug(&format!(
                "Native owner retained during removal: {error}"
            ));
        }
    }

    pub fn next_status_observation_sequence(&self, target_session_id: &str) -> u64 {
        let Ok(mut sequences) = self.status_observation_sequences.lock() else {
            return 0;
        };
        let next = sequences.get(target_session_id).copied().unwrap_or(0) + 1;
        sequences.insert(target_session_id.to_string(), next);
        next
    }

    /// Reserves an accepted status attempt for the exact runtime status Arc.
    /// Hold that Arc's value lock while reserving, so attempts have one order.
    pub fn reserve_status_intent(
        &self,
        session_id: &str,
        current_status: &Arc<std::sync::Mutex<String>>,
        requested_status: &str,
    ) -> u64 {
        let mut revisions = self
            .status_revisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = revisions.entry(session_id.to_string()).or_default();
        let next = session.next();
        let entry = session
            .by_arc
            .entry(Arc::as_ptr(current_status) as usize)
            .or_insert_with(|| StatusArcRevision {
                owner: Arc::downgrade(current_status),
                intent_revision: 0,
                intent_status: String::new(),
                committed_revision: 0,
            });
        entry.owner = Arc::downgrade(current_status);
        entry.intent_revision = next;
        entry.intent_status = requested_status.to_string();
        next
    }

    /// Returns the latest accepted status attempt for the exact runtime Arc.
    pub fn status_intent_revision(
        &self,
        session_id: &str,
        current_status: &Arc<std::sync::Mutex<String>>,
    ) -> u64 {
        self.status_revisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .and_then(|session| session.current(current_status))
            .map(|entry| entry.intent_revision)
            .unwrap_or(0)
    }

    /// Returns the target of the latest accepted status attempt for this Arc.
    pub fn status_intent_status(
        &self,
        session_id: &str,
        current_status: &Arc<std::sync::Mutex<String>>,
    ) -> Option<String> {
        self.status_revisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .and_then(|session| session.current(current_status))
            .map(|entry| entry.intent_status.clone())
    }

    /// Returns the last committed status-value revision for this runtime Arc.
    pub fn status_revision(
        &self,
        session_id: &str,
        current_status: &Arc<std::sync::Mutex<String>>,
    ) -> u64 {
        self.status_revisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .and_then(|session| session.current(current_status))
            .map(|entry| entry.committed_revision)
            .unwrap_or(0)
    }

    /// Records a value mutation; call while holding the exact status Arc lock.
    pub fn commit_status_revision(
        &self,
        session_id: &str,
        current_status: &Arc<std::sync::Mutex<String>>,
        committed_status: &str,
    ) -> u64 {
        let mut revisions = self
            .status_revisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = revisions.entry(session_id.to_string()).or_default();
        let next = session.next();
        session.by_arc.insert(
            Arc::as_ptr(current_status) as usize,
            StatusArcRevision {
                owner: Arc::downgrade(current_status),
                intent_revision: next,
                intent_status: committed_status.to_string(),
                committed_revision: next,
            },
        );
        next
    }

    pub fn set_terminal_theme(&self, theme: &str) {
        if let Ok(mut current) = self.terminal_theme.write() {
            *current = normalize_terminal_theme(theme);
        }
    }

    pub fn terminal_theme(&self) -> String {
        self.terminal_theme
            .read()
            .map(|theme| theme.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    pub fn remote_agent_roster_snapshot(&self) -> Option<Vec<RemoteAgentSummary>> {
        self.remote_agent_roster_cache
            .read()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    pub fn set_remote_agent_roster_snapshot(&self, snapshot: Vec<RemoteAgentSummary>) {
        let mut cached = self
            .remote_agent_roster_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cached = Some(snapshot);
    }

    pub fn set_remote_agent_status(&self, session_id: &str, status: &str, sequence: u64) {
        let mut cached = self
            .remote_agent_status_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cached
            .get(session_id)
            .is_none_or(|(cached_sequence, _)| sequence >= *cached_sequence)
        {
            cached.insert(session_id.to_string(), (sequence, status.to_string()));
        }
    }

    pub fn remote_agent_status(&self, session_id: &str) -> Option<String> {
        self.remote_agent_status_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .map(|(_, status)| status.clone())
    }

    pub fn remote_agent_statuses(&self) -> HashMap<String, String> {
        self.remote_agent_status_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .into_iter()
            .map(|(session_id, (_, status))| (session_id, status))
            .collect()
    }

    pub fn remote_inbox_runtime_items(&self) -> Option<Vec<serde_json::Value>> {
        self.remote_inbox_runtime_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn remote_inbox_runtime_refresh_failed(&self) -> bool {
        self.remote_inbox_runtime_refresh_failed
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn try_start_remote_inbox_runtime_refresh(&self) -> Option<u64> {
        const REFRESH_INTERVAL_MS: i64 = 5_000;
        let now = chrono::Utc::now().timestamp_millis();
        let refreshed_at = self
            .remote_inbox_runtime_refreshed_at
            .load(std::sync::atomic::Ordering::Acquire);
        if refreshed_at > 0 && now.saturating_sub(refreshed_at) < REFRESH_INTERVAL_MS {
            return None;
        }
        self.remote_inbox_runtime_refreshing
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
            .then(|| {
                self.remote_inbox_runtime_generation
                    .load(std::sync::atomic::Ordering::Acquire)
            })
    }

    pub fn set_remote_inbox_runtime_items(&self, generation: u64, items: Vec<serde_json::Value>) {
        let mut cached = self
            .remote_inbox_runtime_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self
            .remote_inbox_runtime_generation
            .load(std::sync::atomic::Ordering::Acquire)
            == generation
        {
            *cached = Some(items);
            self.remote_inbox_runtime_refresh_failed
                .store(false, std::sync::atomic::Ordering::Release);
            self.remote_inbox_runtime_refreshed_at.store(
                chrono::Utc::now().timestamp_millis(),
                std::sync::atomic::Ordering::Release,
            );
        }
        self.remote_inbox_runtime_refreshing
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn fail_remote_inbox_runtime_refresh(&self, generation: u64) {
        if self
            .remote_inbox_runtime_generation
            .load(std::sync::atomic::Ordering::Acquire)
            == generation
        {
            self.remote_inbox_runtime_refresh_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.remote_inbox_runtime_refreshing
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn invalidate_remote_inbox_runtime(&self) {
        let mut cached = self
            .remote_inbox_runtime_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.remote_inbox_runtime_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        *cached = None;
        self.remote_inbox_runtime_refreshed_at
            .store(0, std::sync::atomic::Ordering::Release);
        self.remote_inbox_runtime_refresh_failed
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

fn normalize_terminal_theme(theme: &str) -> String {
    match theme.trim() {
        "light" => "light".to_string(),
        _ => "dark".to_string(),
    }
}

impl Default for AppState {
    fn default() -> Self {
        let mut sys = sysinfo::System::new_all();
        sys.refresh_all();
        Self {
            workbench_io_lock: Mutex::new(()),
            queue_io_lock: Mutex::new(()),
            queue_loaded_snapshot: Mutex::new(None),
            agents: Mutex::new(HashMap::new()),
            system_metrics: Arc::new(Mutex::new(sys)),
            agent_order: Mutex::new(Vec::new()),
            agent_name_reservations: Mutex::new(HashSet::new()),
            agent_lifecycle_locks: Mutex::new(HashMap::new()),
            delivery_locks: Mutex::new(HashMap::new()),
            status_observation_sequences: std::sync::Mutex::new(HashMap::new()),
            status_revisions: std::sync::Mutex::new(HashMap::new()),
            automation_scheduler_handle: Mutex::new(None),
            automation_schedules_paused: std::sync::atomic::AtomicBool::new(false),
            git_watchers: Mutex::new(HashMap::new()),
            library_watchers: Mutex::new(HashMap::new()),
            explorer_watchers: Mutex::new(HashMap::new()),
            file_resources: FileResourceRuntime::default(),
            artifact_runtime: Arc::new(ArtifactRuntime::default()),
            user_terminal: Mutex::new(None),
            interactions: Arc::new(InteractionState::default()),
            native_delivery: Arc::new(crate::delivery::native_broker::NativeDeliveryBroker::new()),
            conversation_capture_policy_lock: Default::default(),
            conversation_archive: ConversationArchiveState::default(),
            clears_in_flight: Default::default(),
            change_snapshots: ChangeSnapshotRuntime::new(),
            remote_runtime: Mutex::new(crate::remote::models::RemoteRuntimeState::default()),
            remote_listener: crate::remote::listener::RemoteListener::default(),
            remote_gateway_config_lock: Mutex::new(()),
            remote_agent_roster_cache: RwLock::new(None),
            remote_agent_status_cache: RwLock::new(HashMap::new()),
            remote_inbox_runtime_cache: RwLock::new(None),
            remote_inbox_runtime_refreshing: std::sync::atomic::AtomicBool::new(false),
            remote_inbox_runtime_refreshed_at: std::sync::atomic::AtomicI64::new(0),
            remote_inbox_runtime_refresh_failed: std::sync::atomic::AtomicBool::new(false),
            remote_inbox_runtime_generation: std::sync::atomic::AtomicU64::new(0),
            terminal_theme: RwLock::new("dark".to_string()),
            terminal_sessions: Arc::new(TerminalSessionBroker::default()),
            // Profiles and downloads live under Wardian home so an isolated
            // WARDIAN_HOME test run cannot collide with production browser state.
            browser_sessions: Arc::new(BrowserSessionBroker::new(
                crate::utils::fs::get_wardian_home()
                    .map(|home| home.join("browser"))
                    .unwrap_or_else(|| std::env::temp_dir().join("wardian-browser")),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_constructs_without_panic() {
        let state = AppState::new();
        assert!(state.agent_order.blocking_lock().is_empty());
        assert!(state.workbench_io_lock.try_lock().is_ok());
        assert!(state.queue_io_lock.try_lock().is_ok());
        assert!(state.queue_loaded_snapshot.try_lock().is_ok());
        assert!(state.conversation_capture_policy_lock.try_lock().is_ok());
        assert!(state
            .terminal_sessions
            .subscribe_wakeups()
            .try_recv()
            .is_err());
        assert_eq!(state.terminal_theme(), "dark");
        assert!(!state
            .automation_schedules_paused
            .load(std::sync::atomic::Ordering::SeqCst));
        drop(state);
    }

    #[test]
    fn terminal_theme_tracks_frontend_effective_theme() {
        let state = AppState::new();

        state.set_terminal_theme("light");
        assert_eq!(state.terminal_theme(), "light");

        state.set_terminal_theme("system");
        assert_eq!(state.terminal_theme(), "dark");
    }

    #[test]
    fn remote_agent_status_cache_rejects_out_of_order_observations() {
        let state = AppState::new();

        state.set_remote_agent_status("agent-1", "Idle", 2);
        state.set_remote_agent_status("agent-1", "Processing", 1);

        assert_eq!(
            state.remote_agent_status("agent-1").as_deref(),
            Some("Idle")
        );
    }
}
