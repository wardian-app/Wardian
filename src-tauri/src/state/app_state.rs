use crate::state::active_agent::ActiveAgent;
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

pub struct AppState {
    // Map of session_id to ActiveAgent
    pub agents: Mutex<HashMap<String, ActiveAgent>>,
    pub system_metrics: Arc<Mutex<sysinfo::System>>,
    pub agent_order: Mutex<Vec<String>>,
    pub agent_name_reservations: Mutex<HashSet<String>>,
    pub agent_lifecycle_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub delivery_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
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
    // Active git repo watchers keyed by workspace path
    pub git_watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
    // Active library watchers keyed by library type, shared by mounted UI consumers
    pub library_watchers: Mutex<HashMap<String, LibraryWatchRegistration>>,
    // Single standalone terminal session for the human user.
    pub user_terminal: Mutex<Option<crate::state::UserTerminalSession>>,
    // Live-only structured ask/reply requests keyed by backend-owned request id.
    pub ask_requests: Mutex<HashMap<String, AskRequestRecord>>,
    // Live-only remote-control authentication and ticket records.
    pub remote_runtime: Mutex<crate::remote::models::RemoteRuntimeState>,
    // Last frontend-reported PTY size per session. Used to open a freshly-spawned
    // PTY at the user's actual terminal dimensions instead of the 80x24 default,
    // which otherwise causes deformed/duplicated TUI output across clear/resume.
    pub pty_sizes: RwLock<HashMap<String, (u16, u16)>>,
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

    pub async fn delivery_lock_for(&self, target_session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.delivery_locks.lock().await;
        locks
            .entry(target_session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn remove_agent_delivery_state(&self, target_session_id: &str) {
        self.delivery_locks.lock().await.remove(target_session_id);
        self.mailbox
            .lock()
            .await
            .remove_for_target(target_session_id);
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
            mailbox: Mutex::new(MailboxState::default()),
            input_senders: RwLock::new(HashMap::new()),
            workflow_triggers: Mutex::new(HashMap::new()),
            workflow_runs: Mutex::new(HashMap::new()),
            triggers_paused: std::sync::atomic::AtomicBool::new(false),
            scheduler_handle: Mutex::new(None),
            git_watchers: Mutex::new(HashMap::new()),
            library_watchers: Mutex::new(HashMap::new()),
            user_terminal: Mutex::new(None),
            ask_requests: Mutex::new(HashMap::new()),
            remote_runtime: Mutex::new(crate::remote::models::RemoteRuntimeState::default()),
            pty_sizes: RwLock::new(HashMap::new()),
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
        drop(state);
    }

    #[tokio::test]
    async fn removing_agent_delivery_state_prunes_lock_and_mailbox_records() {
        let state = AppState::new();
        let _lock = state.delivery_lock_for("agent-1").await;
        state.mailbox.lock().await.enqueue(MailboxMessageDraft {
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
