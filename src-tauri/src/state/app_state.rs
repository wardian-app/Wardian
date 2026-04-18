use crate::state::active_agent::ActiveAgent;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

pub struct AppState {
    // Map of session_id to ActiveAgent
    pub agents: Mutex<HashMap<String, ActiveAgent>>,
    pub system_metrics: Arc<Mutex<sysinfo::System>>,
    pub agent_order: Mutex<Vec<String>>,
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
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
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
            input_senders: RwLock::new(HashMap::new()),
            workflow_triggers: Mutex::new(HashMap::new()),
            workflow_runs: Mutex::new(HashMap::new()),
            triggers_paused: std::sync::atomic::AtomicBool::new(false),
            scheduler_handle: Mutex::new(None),
            git_watchers: Mutex::new(HashMap::new()),
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
        drop(state);
    }
}
