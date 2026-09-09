use crate::remote::models::RemoteAgentSummary;
use crate::state::active_agent::ActiveAgent;
use crate::state::artifact_runtime::ArtifactRuntime;
use crate::state::browser_session::BrowserSessionBroker;
use crate::state::change_snapshot_runtime::ChangeSnapshotRuntime;
use crate::state::conversation_archive::ConversationArchiveState;
use crate::state::file_resources::FileResourceRuntime;
use crate::state::interactions::InteractionState;
use crate::state::mailbox::MailboxState;
use crate::state::terminal_session::TerminalSessionBroker;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use wardian_core::control::{
    DeliveryTransportKind, InteractionDeliveryAttemptRecord, InteractionStatus,
    MailboxDeliveryPhase, MailboxMessageRecord, MailboxMessageStatus, StructuredReply,
};

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
    pub mailbox: Mutex<MailboxState>,
    // Map of automation_id to a list of background trigger handles
    pub automation_triggers: Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>,
    // Map of automation_id to running execution handles
    pub automation_runs: Mutex<HashMap<String, Vec<tauri::async_runtime::JoinHandle<()>>>>,
    pub triggers_paused: std::sync::atomic::AtomicBool,
    pub scheduler_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
    pub ask_requests: Mutex<HashMap<String, AskRequestRecord>>,
    pub interactions: InteractionState,
    /// Wardian-owned persistent provider-session actors. Provider identities
    /// remain generation-bound diagnostics behind this broker.
    pub native_delivery: Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    /// Orders provider-log policy observations before per-agent archive cursor
    /// commits. Callers must snapshot the global agent roster before taking
    /// this gate, then acquire per-agent archive locks only after it.
    pub conversation_capture_policy_lock: Mutex<()>,
    pub conversation_archive: ConversationArchiveState,
    // Serializes and coalesces per-turn change snapshots, one slot per workspace.
    pub change_snapshots: ChangeSnapshotRuntime,
    // Live-only remote-control authentication and ticket records.
    pub remote_runtime: Mutex<crate::remote::models::RemoteRuntimeState>,
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

#[derive(Debug, Clone)]
pub struct AskRequestRecord {
    pub request_id: String,
    pub target_session_id: String,
    pub created_at: String,
    pub reply: Option<StructuredReply>,
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
        self.remote_agent_status_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(target_session_id);
        self.mailbox
            .lock()
            .await
            .remove_for_target(target_session_id);
        self.interactions
            .clear_provider_input_state_in_memory(target_session_id)
            .await;
        self.native_delivery.dispose_agent(target_session_id).await;
    }

    /// Restores durable mailbox work after interaction state has been hydrated.
    ///
    /// Pending records have not crossed the terminal-input boundary and can be
    /// retried after restart. In-flight records may already have populated a
    /// composer, so recovery fails them unless a provider-acceptance receipt
    /// was durably recorded; replaying an ambiguous payload would duplicate it.
    pub async fn hydrate_mailbox_from_persistence(&self) {
        let Ok(records) = wardian_core::db::list_mailbox_messages() else {
            return;
        };

        let mut restored = Vec::with_capacity(records.len());
        for mut record in records {
            let interrupted_dispatch = record.status == MailboxMessageStatus::InFlight;
            match mailbox_recovery_status(&record) {
                MailboxRecoveryStatus::Delivered => {
                    let _ = wardian_core::db::delete_mailbox_message(&record.id);
                    let _ = self
                        .interactions
                        .update_message_status_durable(
                            &record.interaction_id,
                            InteractionStatus::Delivered,
                        )
                        .await;
                }
                MailboxRecoveryStatus::Failed => {
                    if interrupted_dispatch {
                        let generation = self
                            .interactions
                            .current_provider_input_generation(&record.target_session_id)
                            .await
                            .unwrap_or(0);
                        let _ = self
                            .interactions
                            .record_delivery_attempt_durable(
                                &record.interaction_id,
                                &record.target_session_id,
                                DeliveryTransportKind::LiveSurface,
                                generation,
                                "mailbox_recovery",
                                "failed",
                                Some("delivery_interrupted".to_string()),
                                Some("no_provider_acceptance_receipt".to_string()),
                                Some(
                                    "Wardian stopped during terminal delivery; the message was not replayed to prevent duplicate input."
                                        .to_string(),
                                ),
                                None,
                            )
                            .await;
                    }
                    let _ = wardian_core::db::delete_mailbox_message(&record.id);
                    let _ = self
                        .interactions
                        .update_message_status_durable(
                            &record.interaction_id,
                            InteractionStatus::Failed,
                        )
                        .await;
                }
                MailboxRecoveryStatus::Retry => {
                    if record.status == MailboxMessageStatus::InFlight {
                        record.status = MailboxMessageStatus::Pending;
                        record.phase = MailboxDeliveryPhase::Queued;
                        let _ = wardian_core::db::upsert_mailbox_message(&record);
                    }
                    let _ = self
                        .interactions
                        .update_message_status_durable(
                            &record.interaction_id,
                            InteractionStatus::Queued,
                        )
                        .await;
                    restored.push(record);
                }
            }
        }
        self.mailbox.lock().await.hydrate(restored);
    }

    pub fn next_status_observation_sequence(&self, target_session_id: &str) -> u64 {
        let Ok(mut sequences) = self.status_observation_sequences.lock() else {
            return 0;
        };
        let next = sequences.get(target_session_id).copied().unwrap_or(0) + 1;
        sequences.insert(target_session_id.to_string(), next);
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
            self.remote_inbox_runtime_refreshed_at.store(
                chrono::Utc::now().timestamp_millis(),
                std::sync::atomic::Ordering::Release,
            );
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
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MailboxRecoveryStatus {
    Retry,
    Delivered,
    Failed,
}

fn mailbox_recovery_status(record: &MailboxMessageRecord) -> MailboxRecoveryStatus {
    match record.status {
        MailboxMessageStatus::Pending => return MailboxRecoveryStatus::Retry,
        MailboxMessageStatus::Delivered => return MailboxRecoveryStatus::Delivered,
        MailboxMessageStatus::Failed => return MailboxRecoveryStatus::Failed,
        MailboxMessageStatus::InFlight => {}
    }
    let Ok(attempts) = wardian_core::db::list_interaction_delivery_attempts(&record.interaction_id)
    else {
        return MailboxRecoveryStatus::Failed;
    };
    if attempts
        .last()
        .is_some_and(mailbox_attempt_has_provider_acceptance)
    {
        MailboxRecoveryStatus::Delivered
    } else {
        MailboxRecoveryStatus::Failed
    }
}

fn mailbox_attempt_has_provider_acceptance(attempt: &InteractionDeliveryAttemptRecord) -> bool {
    attempt.delivery_state == "provider_accepted"
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
            mailbox: Mutex::new(MailboxState::default()),
            automation_triggers: Mutex::new(HashMap::new()),
            automation_runs: Mutex::new(HashMap::new()),
            triggers_paused: std::sync::atomic::AtomicBool::new(false),
            scheduler_handle: Mutex::new(None),
            automation_scheduler_handle: Mutex::new(None),
            automation_schedules_paused: std::sync::atomic::AtomicBool::new(false),
            git_watchers: Mutex::new(HashMap::new()),
            library_watchers: Mutex::new(HashMap::new()),
            explorer_watchers: Mutex::new(HashMap::new()),
            file_resources: FileResourceRuntime::default(),
            artifact_runtime: Arc::new(ArtifactRuntime::default()),
            user_terminal: Mutex::new(None),
            ask_requests: Mutex::new(HashMap::new()),
            interactions: InteractionState::default(),
            native_delivery: Arc::new(crate::delivery::native_broker::NativeDeliveryBroker::new()),
            conversation_capture_policy_lock: Mutex::new(()),
            conversation_archive: ConversationArchiveState::default(),
            change_snapshots: ChangeSnapshotRuntime::new(),
            remote_runtime: Mutex::new(crate::remote::models::RemoteRuntimeState::default()),
            remote_agent_roster_cache: RwLock::new(None),
            remote_agent_status_cache: RwLock::new(HashMap::new()),
            remote_inbox_runtime_cache: RwLock::new(None),
            remote_inbox_runtime_refreshing: std::sync::atomic::AtomicBool::new(false),
            remote_inbox_runtime_refreshed_at: std::sync::atomic::AtomicI64::new(0),
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
    use crate::state::mailbox::MailboxMessageDraft;
    use wardian_core::control::{
        DeliveryTransportKind, InteractionBodyRef, MessageInputMode, QueuePolicy,
    };

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

    #[tokio::test]
    async fn removing_agent_delivery_state_prunes_lock_and_mailbox_records() {
        let state = AppState::new();
        let _lock = state.delivery_lock_for("agent-1").await;
        state.mailbox.lock().await.enqueue(MailboxMessageDraft {
            interaction_id: "int-agent-1-queued".to_string(),
            target_session_id: "agent-1".to_string(),
            body: "queued".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
        });

        state.remove_agent_delivery_state("agent-1").await;

        assert!(!state.delivery_locks.lock().await.contains_key("agent-1"));
        assert!(state
            .mailbox
            .lock()
            .await
            .list_for_target("agent-1")
            .is_empty());
    }

    #[tokio::test]
    async fn mailbox_hydration_restores_pending_delivery_after_a_restart() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let seeded = AppState::new();
        let interaction = seeded
            .interactions
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "deliver later".to_string(),
                },
            )
            .await
            .unwrap();
        let record = MailboxMessageRecord {
            id: "msg_0000000000001_000001".to_string(),
            interaction_id: interaction.id.clone(),
            target_session_id: "agent-1".to_string(),
            body: "deliver later".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
            created_at: "2026-08-01T00:00:00.000Z".to_string(),
            status: MailboxMessageStatus::Pending,
            phase: MailboxDeliveryPhase::Queued,
        };
        wardian_core::db::upsert_mailbox_message(&record).unwrap();

        let restored = AppState::new();
        restored.interactions.hydrate_from_persistence().await;
        restored.hydrate_mailbox_from_persistence().await;

        assert_eq!(
            restored.mailbox.lock().await.list_for_target("agent-1"),
            vec![record]
        );
        assert_eq!(
            restored
                .interactions
                .interaction(&interaction.id)
                .await
                .unwrap()
                .status,
            InteractionStatus::Queued
        );
    }

    #[tokio::test]
    async fn mailbox_hydration_accepts_a_message_with_a_provider_receipt() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let seeded = AppState::new();
        let interaction = seeded
            .interactions
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "provider confirmed".to_string(),
                },
            )
            .await
            .unwrap();
        let record = MailboxMessageRecord {
            id: "msg_0000000000001_000002".to_string(),
            interaction_id: interaction.id.clone(),
            target_session_id: "agent-1".to_string(),
            body: "provider confirmed".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
            created_at: "2026-08-01T00:00:00.000Z".to_string(),
            status: MailboxMessageStatus::InFlight,
            phase: MailboxDeliveryPhase::Dispatching,
        };
        wardian_core::db::upsert_mailbox_message(&record).unwrap();
        wardian_core::db::upsert_interaction_delivery_attempt(&InteractionDeliveryAttemptRecord {
            id: "attempt_provider_receipt".to_string(),
            interaction_id: interaction.id.clone(),
            target_session_id: "agent-1".to_string(),
            transport: DeliveryTransportKind::LiveSurface,
            generation: 1,
            runtime_state: "mailbox_drain".to_string(),
            delivery_state: "provider_accepted".to_string(),
            delivery_phase: Some("turn_started".to_string()),
            observed_state: Some("turn_started".to_string()),
            reason: None,
            error: None,
            created_at: "2026-08-01T00:00:01.000Z".to_string(),
            updated_at: "2026-08-01T00:00:01.000Z".to_string(),
        })
        .unwrap();

        let restored = AppState::new();
        restored.interactions.hydrate_from_persistence().await;
        restored.hydrate_mailbox_from_persistence().await;

        assert!(restored.mailbox.lock().await.all().is_empty());
        assert!(wardian_core::db::list_mailbox_messages()
            .unwrap()
            .is_empty());
        assert_eq!(
            restored
                .interactions
                .interaction(&interaction.id)
                .await
                .unwrap()
                .status,
            InteractionStatus::Delivered
        );
    }

    #[tokio::test]
    async fn mailbox_hydration_fails_an_unconfirmed_in_flight_delivery_without_replay() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let seeded = AppState::new();
        let interaction = seeded
            .interactions
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "could still be in the composer".to_string(),
                },
            )
            .await
            .unwrap();
        let record = MailboxMessageRecord {
            id: "msg_0000000000001_000003".to_string(),
            interaction_id: interaction.id.clone(),
            target_session_id: "agent-1".to_string(),
            body: "could still be in the composer".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
            created_at: "2026-08-01T00:00:00.000Z".to_string(),
            status: MailboxMessageStatus::InFlight,
            phase: MailboxDeliveryPhase::Dispatching,
        };
        wardian_core::db::upsert_mailbox_message(&record).unwrap();
        wardian_core::db::upsert_interaction_delivery_attempt(&InteractionDeliveryAttemptRecord {
            id: "attempt_submit_unconfirmed".to_string(),
            interaction_id: interaction.id.clone(),
            target_session_id: "agent-1".to_string(),
            transport: DeliveryTransportKind::LiveSurface,
            generation: 1,
            runtime_state: "mailbox_drain".to_string(),
            delivery_state: "submit_sent_unconfirmed".to_string(),
            delivery_phase: Some("submit_key_sent".to_string()),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
            error: None,
            created_at: "2026-08-01T00:00:01.000Z".to_string(),
            updated_at: "2026-08-01T00:00:01.000Z".to_string(),
        })
        .unwrap();

        let restored = AppState::new();
        restored.interactions.hydrate_from_persistence().await;
        restored.hydrate_mailbox_from_persistence().await;

        assert!(restored.mailbox.lock().await.all().is_empty());
        assert!(wardian_core::db::list_mailbox_messages()
            .unwrap()
            .is_empty());
        assert_eq!(
            restored
                .interactions
                .interaction(&interaction.id)
                .await
                .unwrap()
                .status,
            InteractionStatus::Failed
        );
        assert!(
            wardian_core::db::list_interaction_delivery_attempts(&interaction.id)
                .unwrap()
                .iter()
                .any(|attempt| {
                    attempt.delivery_state == "failed"
                        && attempt.delivery_phase.as_deref() == Some("delivery_interrupted")
                })
        );
    }
}
