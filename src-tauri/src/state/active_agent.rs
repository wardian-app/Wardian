use crate::models::AgentConfig;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct ActiveAgent {
    pub config: AgentConfig,
    pub child_process: Option<Box<dyn portable_pty::Child + Send>>,
    pub pty_master: Option<Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>>,
    pub stdin_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    /// Drain-on-read output buffer. The reader thread pushes PTY output here;
    /// the frontend polls via `read_agent_pty` which takes and clears it.
    pub output_buffer: Arc<Mutex<String>>,
    pub process_id: Option<u32>,
    pub query_count: Arc<Mutex<usize>>,
    pub init_timestamp: Arc<Mutex<Option<String>>>,
    pub current_status: Arc<Mutex<String>>,
    pub log_path: Arc<Mutex<Option<PathBuf>>>,
    #[cfg(windows)]
    pub job_object: Option<win32job::Job>,
}
