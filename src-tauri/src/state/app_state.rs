use crate::state::active_agent::ActiveAgent;
use crate::state::conversation_archive::ConversationArchiveState;
use crate::state::interactions::InteractionState;
use crate::state::mailbox::MailboxState;
use crate::state::terminal_attach::TerminalAttachState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use wardian_core::control::StructuredReply;

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
    // Map of session_id to ActiveAgent
    pub agents: Mutex<HashMap<String, ActiveAgent>>,
    pub system_metrics: Arc<Mutex<sysinfo::System>>,
    pub agent_order: Mutex<Vec<String>>,
    pub agent_name_reservations: Mutex<HashSet<String>>,
    pub agent_lifecycle_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub delivery_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    // Per-session PTY resize gates. They keep same-agent resize dedup checks
    // and native resizes ordered without blocking unrelated agent state work.
    pub pty_resize_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    // Process-wide native PTY resize gate. This prevents parallel ConPTY
    // ResizePseudoConsole calls across different terminal sessions.
    pub pty_native_resize_lock: Arc<Mutex<()>>,
    pub status_observation_sequences: std::sync::Mutex<HashMap<String, u64>>,
    pub mailbox: Mutex<MailboxState>,
    // Separate, lightweight map for stdin senders — completely independent from the
    // agents lock. Uses std::sync::RwLock for zero-contention reads from any thread.
    pub input_senders: RwLock<HashMap<String, tokio::sync::mpsc::Sender<Vec<u8>>>>,
    // Map of workflow_id to a list of background trigger handles
    pub workflow_triggers: Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>,
    // Map of workflow_id to running execution handles
    pub workflow_runs: Mutex<HashMap<String, Vec<tauri::async_runtime::JoinHandle<()>>>>,
    pub triggers_paused: std::sync::atomic::AtomicBool,
    pub scheduler_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub workflow_scheduler_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub workflow_schedules_paused: std::sync::atomic::AtomicBool,
    // Active git repo watchers keyed by workspace path
    pub git_watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
    // Active library watchers keyed by library type, shared by mounted UI consumers
    pub library_watchers: Mutex<HashMap<String, LibraryWatchRegistration>>,
    // Active explorer root watchers keyed by normalized root path
    pub explorer_watchers: Mutex<HashMap<String, ExplorerWatchRegistration>>,
    // Single standalone terminal session for the human user.
    pub user_terminal: Mutex<Option<crate::state::UserTerminalSession>>,
    // Live-only structured ask/reply requests keyed by backend-owned request id.
    pub ask_requests: Mutex<HashMap<String, AskRequestRecord>>,
    pub interactions: InteractionState,
    pub conversation_archive: ConversationArchiveState,
    // Live-only remote-control authentication and ticket records.
    pub remote_runtime: Mutex<crate::remote::models::RemoteRuntimeState>,
    // Last frontend-reported PTY size per session. Used to open a freshly-spawned
    // PTY at the user's actual terminal dimensions instead of the 80x24 default,
    // which otherwise causes deformed/duplicated TUI output across clear/resume.
    pub pty_sizes: RwLock<HashMap<String, (u16, u16)>>,
    // Last frontend-reported effective theme. The frontend resolves "system"
    // before updating this so native PTY fallbacks can answer light/dark probes.
    pub terminal_theme: RwLock<String>,
    // Lazy remote terminal attach state. This remains idle unless a remote
    // terminal opens an interactive attachment for an agent.
    pub terminal_attach: Arc<TerminalAttachState>,
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

    pub async fn pty_resize_lock_for(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.pty_resize_locks.lock().await;
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn remove_agent_delivery_state(&self, target_session_id: &str) {
        self.delivery_locks.lock().await.remove(target_session_id);
        self.pty_resize_locks.lock().await.remove(target_session_id);
        if let Ok(mut sequences) = self.status_observation_sequences.lock() {
            sequences.remove(target_session_id);
        }
        self.mailbox
            .lock()
            .await
            .remove_for_target(target_session_id);
        self.interactions
            .clear_provider_input_state(target_session_id)
            .await;
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
            agents: Mutex::new(HashMap::new()),
            system_metrics: Arc::new(Mutex::new(sys)),
            agent_order: Mutex::new(Vec::new()),
            agent_name_reservations: Mutex::new(HashSet::new()),
            agent_lifecycle_locks: Mutex::new(HashMap::new()),
            delivery_locks: Mutex::new(HashMap::new()),
            pty_resize_locks: Mutex::new(HashMap::new()),
            pty_native_resize_lock: Arc::new(Mutex::new(())),
            status_observation_sequences: std::sync::Mutex::new(HashMap::new()),
            mailbox: Mutex::new(MailboxState::default()),
            input_senders: RwLock::new(HashMap::new()),
            workflow_triggers: Mutex::new(HashMap::new()),
            workflow_runs: Mutex::new(HashMap::new()),
            triggers_paused: std::sync::atomic::AtomicBool::new(false),
            scheduler_handle: Mutex::new(None),
            workflow_scheduler_handle: Mutex::new(None),
            workflow_schedules_paused: std::sync::atomic::AtomicBool::new(false),
            git_watchers: Mutex::new(HashMap::new()),
            library_watchers: Mutex::new(HashMap::new()),
            explorer_watchers: Mutex::new(HashMap::new()),
            user_terminal: Mutex::new(None),
            ask_requests: Mutex::new(HashMap::new()),
            interactions: InteractionState::default(),
            conversation_archive: ConversationArchiveState::default(),
            remote_runtime: Mutex::new(crate::remote::models::RemoteRuntimeState::default()),
            pty_sizes: RwLock::new(HashMap::new()),
            terminal_theme: RwLock::new("dark".to_string()),
            terminal_attach: Arc::new(TerminalAttachState::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mailbox::MailboxMessageDraft;
    use wardian_core::control::{MessageInputMode, QueuePolicy};

    #[test]
    fn app_state_constructs_without_panic() {
        let state = AppState::new();
        assert!(state.agent_order.blocking_lock().is_empty());
        assert!(state.terminal_attach.snapshot("missing-agent").is_none());
        assert_eq!(state.terminal_theme(), "dark");
        assert!(!state
            .workflow_schedules_paused
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
}
