//! Retained source and status snapshot for a single telemetry pass.
use super::*;

pub(super) struct AgentSnapshot {
    pub(super) claude_source: Option<std::path::PathBuf>,
    pub(super) claude_root: Option<std::path::PathBuf>,
    pub(super) claude_launch_cwd: Option<std::path::PathBuf>,
    pub(super) session_id: String,
    pub(super) provider: String,
    pub(super) is_off: bool,
    pub(super) resume_session: Option<String>,
    pub(super) conversation_logging: wardian_core::conversations::AgentConversationLoggingSetting,
    pub(super) capture_conversation: Option<String>,
    pub(super) provider_generation: u64,
    pub(super) process_id: Option<u32>,
    pub(super) query_count: Arc<Mutex<usize>>,
    pub(super) init_timestamp: Arc<Mutex<Option<String>>>,
    pub(super) last_query_timestamp: Arc<Mutex<Option<String>>>,
    pub(super) current_status: Arc<Mutex<String>>,
    pub(super) status_observation: Mutex<TelemetryStatusDraft>,
    pub(super) watch_state: Arc<Mutex<crate::state::AgentWatchState>>,
    pub(super) last_output_at: Arc<Mutex<Option<std::time::SystemTime>>>,
    pub(super) log_path: Arc<Mutex<Option<std::path::PathBuf>>>,
    pub(super) log_last_modified: Arc<Mutex<Option<std::time::SystemTime>>>,
}
