//! Retained stop fences for captured Codex PTY/background runtimes.
//! Lifecycle callers register synchronously before their first await. A waiter
//! never owns the child handles; only observed exit releases the process fence.
use crate::state::terminal_session::{TerminalBrokerError, TerminalSessionBroker};
use crate::state::ActiveAgent;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[cfg(test)]
#[path = "codex_stop/tests.rs"]
mod tests;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key(PathBuf, String);

type Registry = HashMap<Key, Vec<Arc<Entry>>>;
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

#[derive(Clone)]
enum Status {
    Captured,
    Running,
    Failed(String),
    Exited,
}

struct Entry {
    keys: Vec<Key>,
    runtime: Mutex<Option<ActiveAgent>>,
    status: watch::Sender<Status>,
    timeout: Duration,
    terminal: Option<TerminalCleanup>,
}

struct TerminalCleanup {
    broker: Arc<TerminalSessionBroker>,
    executor: tokio::runtime::Handle,
    generation: u64,
}

impl TerminalCleanup {
    fn terminate(&self, agent_id: &str) -> Result<(), String> {
        match self
            .executor
            .block_on(self.broker.terminate_runtime(agent_id, self.generation))
        {
            Ok(()) | Err(TerminalBrokerError::SessionNotFound) => Ok(()),
            Err(error) => Err(format!(
                "Cannot terminate captured Codex terminal generation {}; fence retained: {error}",
                self.generation
            )),
        }
    }
}

/// Validate while the runtime is still in the roster. No ownership has moved
/// if this fails. Caller retains per-agent lifecycle exclusion through capture.
pub(crate) fn prepare_stop(home: &Path, agent_id: &str) -> Result<StopRegistration, String> {
    prepare_with_timeout(home, agent_id, Duration::from_secs(5))
}

fn prepare_with_timeout(
    home: &Path,
    agent_id: &str,
    timeout: Duration,
) -> Result<StopRegistration, String> {
    Ok(StopRegistration {
        key: key(home, agent_id)?,
        timeout,
        terminal: None,
    })
}

#[must_use = "validate before roster removal, then capture synchronously"]
pub(crate) struct StopRegistration {
    key: Key,
    timeout: Duration,
    terminal: Option<(Arc<TerminalSessionBroker>, tokio::runtime::Handle)>,
}

impl StopRegistration {
    /// An uninstalled candidate has no roster owner to clean its terminal actor.
    /// Capture the executor before spawn so cancellation cleanup can await the
    /// exact broker generation from the retained stop worker's own thread.
    pub(crate) fn with_terminal_cleanup(
        mut self,
        broker: Arc<TerminalSessionBroker>,
    ) -> Result<Self, String> {
        let executor = tokio::runtime::Handle::try_current()
            .map_err(|error| format!("Cannot reserve Codex terminal cleanup executor: {error}"))?;
        self.terminal = Some((broker, executor));
        Ok(self)
    }

    /// Infallible, synchronous handoff immediately after roster removal. This
    /// must precede EVERY fallible operation/await after taking ActiveAgent.
    /// The returned guard starts cleanup on Drop, including broker-await abort.
    pub(crate) fn capture(self, agent: ActiveAgent) -> StopGuard {
        let (status, _) = watch::channel(Status::Captured);
        // Keep the terminal identity independently: process exit clears the
        // ActiveAgent field, but must not discard its pending broker cleanup.
        let terminal = self.terminal.and_then(|(broker, executor)| {
            agent.runtime_generation.map(|generation| TerminalCleanup {
                broker,
                executor,
                generation,
            })
        });
        let entry = Arc::new(Entry {
            keys: vec![self.key],
            runtime: Mutex::new(Some(agent)),
            status,
            timeout: self.timeout,
            terminal,
        });
        let guard = StopGuard(Some(entry.clone()));
        let mut map = registry().lock().unwrap_or_else(|error| error.into_inner());
        for key in &entry.keys {
            map.entry(key.clone()).or_default().push(entry.clone());
        }
        guard
    }
}

#[must_use = "hold across broker shutdown, then begin_stop and wait"]
pub(crate) struct StopGuard(Option<Arc<Entry>>);

impl StopGuard {
    pub(crate) fn begin_stop(mut self) -> StopHandle {
        let entry = self.0.take().expect("captured stop guard");
        start(&entry, true);
        StopHandle(entry)
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        if let Some(entry) = self.0.take() {
            start(&entry, true);
        }
    }
}

#[derive(Clone)]
pub(crate) struct StopHandle(Arc<Entry>);

impl StopHandle {
    /// Cancellation only drops this observer. The registry and cleanup worker
    /// retain the runtime, including after timeout, kill/wait error or panic.
    pub(crate) async fn wait(&self) -> Result<(), String> {
        wait(&self.0).await
    }
}

/// Call before new launch/migration/deletion under existing lifecycle exclusion.
/// Captured/running stops wait; failed stops remain fenced and return their error.
/// No stop is implicitly retried, and no native message or provider work is replayed.
pub(crate) async fn await_quiescent(home: &Path, agent_id: &str) -> Result<(), String> {
    let key = key(home, agent_id)?;
    loop {
        let entries = entries(&key);
        if entries.is_empty() {
            return Ok(());
        }
        for entry in entries {
            wait(&entry).await?;
        }
        // Re-read after waiting so another captured incarnation is not erased.
    }
}

/// Explicit retry against the SAME retained handles; call await_quiescent next.
/// A still-captured guard belongs to its lifecycle caller and is not preempted.
pub(crate) fn retry_stop(home: &Path, agent_id: &str) -> Result<(), String> {
    for entry in entries(&key(home, agent_id)?) {
        start(&entry, false);
    }
    Ok(())
}

fn entries(key: &Key) -> Vec<Arc<Entry>> {
    registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(key)
        .cloned()
        .unwrap_or_default()
}

fn key(home: &Path, id: &str) -> Result<Key, String> {
    if !home.is_absolute()
        || id.trim() != id
        || id.contains(['/', '\\'])
        || !matches!(
            Path::new(id).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
    {
        return Err("Codex stop requires an absolute home and one complete agent ID".into());
    }
    let home = std::fs::canonicalize(home)
        .map_err(|error| format!("Cannot identify Codex stop home: {error}"))?;
    #[cfg(windows)]
    let home = PathBuf::from(crate::utils::fs::strip_windows_verbatim_prefix(
        home.to_str().ok_or("Codex stop home is not UTF-8")?,
    ));
    Ok(Key(home, id.to_owned()))
}

async fn wait(entry: &Arc<Entry>) -> Result<(), String> {
    let mut status = entry.status.subscribe();
    loop {
        match status.borrow_and_update().clone() {
            Status::Exited => return Ok(()),
            Status::Failed(error) => return Err(error),
            Status::Captured | Status::Running => {}
        }
        status
            .changed()
            .await
            .map_err(|_| "Codex stop observer closed without exit evidence".to_owned())?;
    }
}

fn start(entry: &Arc<Entry>, allow_captured: bool) {
    let started = entry.status.send_if_modified(|status| {
        if matches!(status, Status::Failed(_))
            || (allow_captured && matches!(status, Status::Captured))
        {
            *status = Status::Running;
            true
        } else {
            false
        }
    });
    if !started {
        return;
    }
    let worker = entry.clone();
    let spawned = std::thread::Builder::new().name("codex-stop".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut runtime = worker.runtime.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(agent) = runtime.as_mut() {
                {
                    let config = agent.config.lock().unwrap_or_else(|error| error.into_inner());
                    if config.session_id != worker.keys[0].1 || config.provider != "codex" {
                        return Err("Captured runtime does not match the reserved Codex identity; runtime retained".into());
                    }
                }
                if let Some(terminal) = &worker.terminal {
                    terminal.terminate(&worker.keys[0].1)?;
                }
                stop_and_join(agent, worker.timeout)?;
            }
            // Terminal cleanup (when captured) and every child exit completed
            // before ActiveAgent Drop and release of the retained stop fence.
            drop(runtime.take());
            Ok(())
        })).unwrap_or_else(|_| Err("Codex stop worker panicked; runtime and fence retained".into()));
        match result {
            Ok(()) => {
                // Remove only this exact entry; a later capture is independent.
                release(&worker);
                worker.status.send_replace(Status::Exited);
            }
            Err(error) => { worker.status.send_replace(Status::Failed(error)); }
        }
    });
    if let Err(error) = spawned {
        entry.status.send_replace(Status::Failed(format!(
            "Cannot start Codex stop worker; runtime retained: {error}"
        )));
    }
}

fn release(entry: &Arc<Entry>) {
    let mut map = registry().lock().unwrap_or_else(|error| error.into_inner());
    for key in &entry.keys {
        if let Some(entries) = map.get_mut(key) {
            entries.retain(|current| !Arc::ptr_eq(current, entry));
        }
        if map.get(key).is_some_and(Vec::is_empty) {
            map.remove(key);
        }
    }
}

fn stop_and_join(agent: &mut ActiveAgent, timeout: Duration) -> Result<(), String> {
    if agent.child_process.is_none()
        && (agent.process_id.is_some() || agent.runtime_generation.is_some())
    {
        return Err(
            "Cannot prove Codex TUI exit: captured child handle is missing; fence retained".into(),
        );
    }
    if poll_exited(agent)? {
        return Ok(());
    }
    let mut failure = None;
    if let Some(child) = agent.child_process.as_mut() {
        #[cfg(windows)]
        if let Some(pid) = child.process_id() {
            if let Err(error) = crate::utils::process::force_kill_process_tree(pid) {
                failure = Some(error);
            }
        }
        if let Err(error) = child.kill() {
            failure = Some(error.to_string());
        }
    }
    for child in &mut agent.background_processes {
        #[cfg(windows)]
        if let Err(error) = crate::utils::process::force_kill_process_tree(child.id()) {
            failure = Some(error);
        }
        if let Err(error) = child.kill() {
            failure = Some(error.to_string());
        }
    }
    #[cfg(windows)]
    {
        agent.job_object.take();
    }
    let deadline = Instant::now() + timeout;
    loop {
        if poll_exited(agent)? {
            return Ok(());
        }
        if let Some(error) = failure {
            return Err(format!(
                "Codex kill failed before observed exit; runtime retained: {error}"
            ));
        }
        if Instant::now() >= deadline {
            return Err("Timed out joining Codex children; runtime and fence retained; explicit retry required".into());
        }
        std::thread::sleep(
            Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn poll_exited(agent: &mut ActiveAgent) -> Result<bool, String> {
    if let Some(child) = agent.child_process.as_mut() {
        if child
            .try_wait()
            .map_err(|error| format!("Cannot observe Codex TUI exit; fence retained: {error}"))?
            .is_some()
        {
            agent.child_process.take();
            agent.process_id = None;
            agent.runtime_generation = None;
        }
    }
    let mut index = 0;
    while index < agent.background_processes.len() {
        if agent.background_processes[index]
            .try_wait()
            .map_err(|error| {
                format!("Cannot observe Codex background exit; fence retained: {error}")
            })?
            .is_some()
        {
            // try_wait already observed/reaped exit; consume its cached status
            // explicitly before releasing the retained child handle.
            agent
                .background_processes
                .remove(index)
                .wait()
                .map_err(|error| {
                    format!("Cannot finalize observed Codex background exit: {error}")
                })?;
        } else {
            index += 1;
        }
    }
    Ok(agent.child_process.is_none() && agent.background_processes.is_empty())
}
