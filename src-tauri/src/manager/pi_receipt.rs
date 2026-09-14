//! Stock Pi interactive receipts. Exact native content is observed before JSONL flush.
//! One serialized pending prompt per launch; identical simultaneous manual input
//! remains the same operational ambiguity as the other native content receipts.
mod child;
mod stream;
#[cfg(test)]
mod tests;

use crate::state::{terminal_session::TerminalSessionBroker, AppState};
use crate::utils::delivery_transaction::TerminalInputSink;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock, Weak,
};
use std::time::{Duration, Instant};
use stream::{Pending, Stream, UserStart, MAX_RECORD_BYTES, MAX_STREAM_BYTES};

type Registry = HashMap<(PathBuf, String), Weak<Receipt>>;
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

/// The hook is the only start/count source; delayed log records still flow to
/// transcript and raw JSON consumers in the caller.
pub(crate) fn log_activity(
    event: wardian_core::models::AgentEvent,
) -> Option<wardian_core::models::AgentEvent> {
    (!matches!(event, wardian_core::models::AgentEvent::UserQuery)).then_some(event)
}

pub(crate) struct Receipt {
    home: PathBuf,
    agent_id: String,
    native_id: String,
    nonce: String,
    directory: PathBuf,
    file: Mutex<File>,
    identity: same_file::Handle,
    state: Mutex<Stream>,
    generation: AtomicU64,
    stopped: AtomicBool,
    child_started: AtomicBool,
    watcher: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Receipt {
    /// All paths are new and launch-owned. No global Pi config or package is changed.
    pub fn prepare(config: &wardian_core::models::AgentConfig) -> Result<Arc<Self>, String> {
        let home = crate::utils::get_wardian_home().ok_or("Wardian home unavailable")?;
        let native = super::session_identity::expected_caller_owned_identity(config)
            .ok_or("Pi receipt requires caller-owned native identity")?;
        let sessions = crate::providers::pi::PiProvider::session_dir(&config.session_id)
            .ok_or("Pi receipt directory unavailable")?;
        let parent = sessions.parent().ok_or("Invalid Pi session directory")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let slot = tempfile::Builder::new()
            .prefix("receipt-")
            .tempdir_in(parent)
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(slot.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let directory = slot.path().canonicalize().map_err(|e| e.to_string())?;
        let path = directory.join("events.jsonl");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|e| e.to_string())?;
        let identity = same_file::Handle::from_file(file.try_clone().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let descriptor =
            serde_json::json!({"event_path":path,"nonce":nonce,"native_session_id":native});
        let extension = format!(
            "const WARDIAN_PI_LAUNCH = {descriptor};\n{}",
            include_str!("pi_receipt/extension.mjs")
        );
        std::fs::write(directory.join("extension.mjs"), extension).map_err(|e| e.to_string())?;
        let _retained_path = slot.keep();
        Ok(Arc::new(Self {
            home,
            agent_id: config.session_id.clone(),
            native_id: native.into(),
            nonce,
            directory,
            file: Mutex::new(file),
            identity,
            state: Mutex::new(Stream::default()),
            generation: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            child_started: AtomicBool::new(false),
            watcher: Mutex::new(None),
        }))
    }

    pub fn append_args(&self, args: &mut Vec<String>) {
        let index = args
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or(args.len());
        args.splice(
            index..index,
            [
                "--extension".into(),
                self.directory
                    .join("extension.mjs")
                    .to_string_lossy()
                    .into_owned(),
            ],
        );
    }

    pub fn own_child(
        self: &Arc<Self>,
        child: Box<dyn portable_pty::Child + Send>,
    ) -> Box<dyn portable_pty::Child + Send> {
        self.child_started.store(true, Ordering::Release);
        Box::new(child::OwnedChild::new(child, self.clone()))
    }

    pub fn bind(self: &Arc<Self>, generation: u64) {
        self.generation.store(generation, Ordering::Release);
        let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, receipt| receipt.strong_count() > 0);
        map.insert(
            (self.home.clone(), self.agent_id.clone()),
            Arc::downgrade(self),
        );
    }

    pub fn retain_watcher(&self, watcher: std::thread::JoinHandle<()>) {
        *self.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher);
    }
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn fail(&self, message: String) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).failure = Some(message);
    }

    fn file_length(&self) -> Result<u64, String> {
        let path = self.directory.join("events.jsonl");
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || same_file::Handle::from_path(&path).map_err(|e| e.to_string())? != self.identity
        {
            return Err("Pi receipt file replaced".into());
        }
        if metadata.len() > MAX_STREAM_BYTES {
            return Err("Pi receipt stream capacity exceeded".into());
        }
        Ok(metadata.len())
    }

    fn read_events(&self) -> Result<Vec<UserStart>, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let length = self.file_length()?;
        if length < state.offset {
            return Err("Pi receipt stream truncated".into());
        }
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        file.seek(SeekFrom::Start(state.offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_STREAM_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_STREAM_BYTES {
            return Err("Pi receipt stream oversized".into());
        }
        let mut events = Vec::new();
        let mut consumed = 0;
        while let Some(end) = bytes[consumed..].iter().position(|b| *b == b'\n') {
            if end > MAX_RECORD_BYTES {
                return Err("Pi receipt record oversized".into());
            }
            let start = state.offset;
            if let Some(event) = state.accept_record(
                &bytes[consumed..consumed + end],
                start,
                &self.nonce,
                &self.native_id,
            )? {
                events.push(event);
            }
            consumed += end + 1;
            state.offset += (end + 1) as u64;
        }
        if bytes.len() - consumed > MAX_RECORD_BYTES {
            return Err("Pi receipt partial record oversized".into());
        }
        Ok(events)
    }

    /// Called by the existing Pi watcher. Generation validation and watch publication
    /// use the same broker read-lock fence as the approved OpenCode receipt.
    pub fn poll(
        &self,
        broker: &TerminalSessionBroker,
        executor: &tokio::runtime::Handle,
        watch: &Arc<Mutex<crate::state::AgentWatchState>>,
        query_count: &Arc<Mutex<usize>>,
        app: &tauri::AppHandle,
        status: &Arc<Mutex<String>>,
    ) {
        if self.stopped() {
            return;
        }
        let result = self.read_events().and_then(|events| {
            for event in events {
                executor
                    .block_on(broker.record_turn_started_for_generation(
                        &self.agent_id,
                        self.generation.load(Ordering::Acquire),
                        watch.clone(),
                    ))
                    .map_err(|e| e.to_string())?;
                *query_count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                super::set_agent_status(app, &self.agent_id, status, "Processing...");
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .published(&event);
            }
            Ok(())
        });
        if let Err(error) = result {
            self.fail(error);
        }
    }

    fn ticket(
        self: &Arc<Self>,
        text: &str,
        broker: Arc<TerminalSessionBroker>,
    ) -> Result<Ticket, String> {
        let mut stream = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped() || !stream.ready || stream.failure.is_some() {
            return Err("Pi receipt not ready; no input sent".into());
        }
        if stream.pending.is_some() {
            return Err("Pi receipt already has a pending prompt".into());
        }
        let offset = self.file_length()?;
        stream.next_pending += 1;
        let id = stream.next_pending;
        stream.pending = Some(Pending {
            id,
            offset,
            digest: format!("{:x}", Sha256::digest(text.as_bytes())),
            bytes: text.len(),
            accepted: false,
        });
        Ok(Ticket {
            receipt: self.clone(),
            id,
            broker,
        })
    }

    // Called only after the owned child exited and the watcher joined. No recursive
    // deletion: unexpected contents or a replaced path are retained for inspection.
    fn cleanup(&self) {
        if self.directory.canonicalize().ok().as_deref() != Some(self.directory.as_path()) {
            return;
        }
        if self.file_length().is_err() {
            return;
        }
        let _ = std::fs::remove_file(self.directory.join("extension.mjs"));
        let _ = std::fs::remove_file(self.directory.join("events.jsonl"));
        let _ = std::fs::remove_dir(&self.directory);
    }
}
impl Drop for Receipt {
    fn drop(&mut self) {
        if !self.child_started.load(Ordering::Acquire) {
            self.cleanup();
        }
    }
}

/// Capture the exact installed runtime and a disk boundary before any payload write.
/// Normalization is the same as the terminal sender; no trim/guess of native events.
pub(crate) async fn arm(state: &AppState, agent_id: &str, prompt: &str) -> Result<Ticket, String> {
    let text = crate::utils::terminal_input::normalize_prompt_for_terminal_submit(prompt);
    if text.is_empty() || text.starts_with('/') || text.starts_with('!') {
        return Err(
            "Pi content receipts require a nonempty plain prompt; commands are not turn receipts"
                .into(),
        );
    }
    let (generation, native) = {
        let agents = state.agents.lock().await;
        let agent = agents.get(agent_id).ok_or("Pi runtime unavailable")?;
        let config = agent.config.lock().map_err(|_| "Pi config poisoned")?;
        (
            agent
                .runtime_generation
                .ok_or("Pi runtime generation unavailable")?,
            super::session_identity::expected_caller_owned_identity(&config)
                .map(str::to_owned)
                .ok_or("Pi native identity unavailable")?,
        )
    };
    let home = crate::utils::get_wardian_home().ok_or("Wardian home unavailable")?;
    let receipt = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(home, agent_id.into()))
        .and_then(Weak::upgrade)
        .ok_or("Pi receipt extension unavailable; restart required")?;
    if receipt.generation.load(Ordering::Acquire) != generation || receipt.native_id != native {
        return Err("Pi receipt belongs to a different runtime".into());
    }
    let ready_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if receipt.stopped() {
            return Err("Pi receipt runtime stopped".into());
        }
        {
            let stream = receipt.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(error) = &stream.failure {
                return Err(error.clone());
            }
            if stream.ready {
                drop(stream);
                return receipt.ticket(&text, state.terminal_sessions.clone());
            }
        }
        if Instant::now() >= ready_deadline {
            return Err("Pi receipt extension did not report ready; no input sent".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(crate) struct Ticket {
    receipt: Arc<Receipt>,
    id: u64,
    broker: Arc<TerminalSessionBroker>,
}
impl Ticket {
    pub async fn wait(&self) -> Result<(), String> {
        self.wait_for(Duration::from_secs(10)).await
    }

    async fn wait_for(&self, timeout: Duration) -> Result<(), String> {
        let start = Instant::now();
        loop {
            if self.receipt.stopped() {
                return Err("Pi exited before confirming input; do not replay".into());
            }
            {
                let stream = self.receipt.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(error) = &stream.failure {
                    return Err(error.clone());
                }
                let pending = stream
                    .pending
                    .as_ref()
                    .filter(|p| p.id == self.id)
                    .ok_or("Pi pending receipt replaced")?;
                if pending.accepted {
                    return Ok(());
                }
            }
            if start.elapsed() >= timeout {
                return Err("Timed out waiting for exact Pi user message start; submission uncertain, do not replay".into());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.receipt.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == self.id)
        {
            state.pending = None;
        }
    }
}
impl TerminalInputSink for Ticket {
    fn send_bytes(
        &self,
        bytes: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            if self.receipt.stopped() {
                return Err("Pi receipt runtime stopped".into());
            }
            let broker = self.broker.clone();
            let agent = self.receipt.agent_id.clone();
            let generation = self.receipt.generation.load(Ordering::Acquire);
            tokio::task::spawn_blocking(move || {
                broker.send_privileged_input_blocking(&agent, generation, bytes)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
        })
    }
}
