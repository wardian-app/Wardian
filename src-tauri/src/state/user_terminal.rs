use std::sync::{Arc, Mutex};

pub struct UserTerminalSession {
    pub session_id: String,
    pub shell_id: String,
    pub child_process: Option<Box<dyn portable_pty::Child + Send>>,
    pub pty_master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    pub stdin_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub output_buffer: Arc<Mutex<String>>,
    pub process_id: Option<u32>,
    pub exited: Arc<Mutex<bool>>,
    #[cfg(windows)]
    pub job_object: Option<win32job::Job>,
}

impl Drop for UserTerminalSession {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if let Some(pid) = self.process_id.take() {
                let _ = crate::utils::process::force_kill_process_tree(pid);
            }
        }

        if let Some(mut child) = self.child_process.take() {
            let _ = child.kill();
        }
    }
}
