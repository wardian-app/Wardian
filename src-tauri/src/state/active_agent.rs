use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wardian_core::models::AgentConfig;

use super::AgentWatchState;

pub struct ActiveAgent {
    // This lock protects short configuration mutations and snapshots only.
    // Callers must clone the configuration and release this guard before any
    // await or blocking I/O; it is deliberately never held across an async
    // suspension. The surrounding agent map uses Tokio's async mutex.
    pub config: Arc<Mutex<AgentConfig>>,
    pub child_process: Option<Box<dyn portable_pty::Child + Send>>,
    pub background_processes: Vec<std::process::Child>,
    /// Agent-bound CLI authority for this exact provider process. Dropping the
    /// lease revokes only this process and leaves concurrent runtimes valid.
    pub memory_capability: Option<wardian_core::memory::MemoryCapabilityLease>,
    /// Exact terminal-session broker generation for this native runtime.
    /// Native master/input handles live only inside the broker actor.
    pub runtime_generation: Option<u64>,
    pub process_id: Option<u32>,
    pub query_count: Arc<Mutex<usize>>,
    pub init_timestamp: Arc<Mutex<Option<String>>>,
    /// Latest provider-derived user-message timestamp retained between
    /// telemetry passes, including passes where the source mtime is unchanged.
    pub last_query_timestamp: Arc<Mutex<Option<String>>>,
    pub current_status: Arc<Mutex<String>>,
    pub last_status_at: Arc<Mutex<Option<String>>>,
    pub watch_state: Arc<Mutex<AgentWatchState>>,
    pub terminal_title: Arc<Mutex<String>>,
    pub last_output_at: Arc<Mutex<Option<std::time::SystemTime>>>,
    pub log_path: Arc<Mutex<Option<PathBuf>>>,
    pub log_last_modified: Arc<Mutex<Option<std::time::SystemTime>>>,
    #[cfg(windows)]
    pub job_object: Option<crate::utils::process::RuntimeProcessJob>,
}

impl Drop for ActiveAgent {
    fn drop(&mut self) {
        // Safety net: if the agent is dropped without explicit termination
        // (e.g. during a panic, or if someone clears the HashMap without calling
        // terminate_active_agent_process first), force-kill the process tree.
        // On macOS/Linux the PTY master drop sends SIGHUP, so this is mainly
        // needed on Windows where ConPTY doesn't propagate termination.
        #[cfg(windows)]
        {
            if let Some(pid) = self.process_id.take().filter(|_| {
                !self
                    .job_object
                    .as_ref()
                    .is_some_and(|job| job.contained_from_launch())
            }) {
                let _ = crate::utils::process::force_kill_process_tree(pid);
            }
        }

        // Kill the PTY child if it wasn't already taken.
        if let Some(mut child) = self.child_process.take() {
            let _ = child.kill();
        }

        for mut child in self.background_processes.drain(..) {
            #[cfg(windows)]
            {
                let _ = crate::utils::process::force_kill_process_tree(child.id());
            }
            let _ = child.kill();
        }
    }
}
