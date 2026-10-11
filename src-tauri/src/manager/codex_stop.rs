//! Retained native-runtime stop fences, independent of current provider selection.
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopPolicy {
    LegacyCodex,
    RetainedChild,
    ContainedJob,
}

struct Entry {
    identity_matches: bool,
    owns_runtime: bool,
    keys: Vec<Key>,
    runtime: Mutex<Option<ActiveAgent>>,
    status: watch::Sender<Status>,
    timeout: Duration,
    terminal: Option<TerminalCleanup>,
    provider: String,
    policy: StopPolicy,
    generation: Option<u64>,
    status_arc: Arc<Mutex<String>>,
    terminal_complete: std::sync::atomic::AtomicBool,
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
                "Cannot terminate captured native terminal generation {}; fence retained: {error}",
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

/// Pi cleanup uses only captured child handles, never a discovered PID tree.
pub(crate) fn prepare_pi_stop(home: &Path, agent_id: &str) -> Result<StopRegistration, String> {
    prepare_runtime_stop(home, agent_id, "pi")
}

/// Reserve native stop identity before launch or roster capture. Windows
/// candidates use contained ownership; existing Codex/Pi uncontained policies
/// remain available, and capture upgrades them when a prelaunch job is present.
pub(crate) fn prepare_runtime_stop(
    home: &Path,
    agent_id: &str,
    provider: &str,
) -> Result<StopRegistration, String> {
    let mut registration = prepare_stop(home, agent_id)?;
    registration.provider = provider.to_owned();
    registration.policy = match provider {
        "codex" => StopPolicy::LegacyCodex,
        "pi" => StopPolicy::RetainedChild,
        _ => StopPolicy::ContainedJob,
    };
    Ok(registration)
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
        provider: "codex".into(),
        policy: StopPolicy::LegacyCodex,
    })
}

#[must_use = "validate before roster removal, then capture synchronously"]
pub(crate) struct StopRegistration {
    key: Key,
    timeout: Duration,
    terminal: Option<(Arc<TerminalSessionBroker>, tokio::runtime::Handle)>,
    provider: String,
    policy: StopPolicy,
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
            .map_err(|error| format!("Cannot reserve native terminal cleanup executor: {error}"))?;
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
        // Snapshot identity and containment before mutable roster config can
        // change. Cleanup never consults a later provider selection.
        let identity_matches = {
            let config = agent
                .config
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            config.session_id == self.key.1 && config.provider == self.provider
        };
        let owns_runtime = agent.child_process.is_some()
            || !agent.background_processes.is_empty()
            || agent.process_id.is_some()
            || agent.runtime_generation.is_some();
        #[cfg(windows)]
        let owns_runtime = owns_runtime || agent.job_object.is_some();
        let policy = self.policy;
        #[cfg(windows)]
        let policy = if agent
            .job_object
            .as_ref()
            .is_some_and(|job| job.contained_from_launch())
        {
            StopPolicy::ContainedJob
        } else {
            policy
        };
        let entry = Arc::new(Entry {
            identity_matches,
            owns_runtime,
            policy,
            keys: vec![self.key],
            generation: agent.runtime_generation,
            status_arc: agent.current_status.clone(),
            runtime: Mutex::new(Some(agent)),
            status,
            timeout: self.timeout,
            terminal,
            provider: self.provider,
            terminal_complete: std::sync::atomic::AtomicBool::new(false),
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
    pub(crate) fn provider_label(&self) -> &str {
        match self.0.provider.as_str() {
            "pi" => "Pi",
            "opencode" => "OpenCode",
            "codex" => "Codex",
            provider => provider,
        }
    }

    /// Cancellation only drops this observer. The registry and cleanup worker
    /// retain the runtime, including after timeout, kill/wait error or panic.
    pub(crate) async fn wait(&self) -> Result<(), String> {
        wait(&self.0).await?;
        // A cancelled Pi registration can await cleanup before its startup
        // owner observes capture. Preserve the exact exit receipt for that owner.
        if self.0.provider != "pi" {
            release(&self.0);
        }
        Ok(())
    }

    /// Observe a later exit after an uncertain stop, without issuing another
    /// kill. Failed terminal cleanup still requires an explicit lifecycle retry.
    pub(crate) fn observe_exit(&self) -> Result<bool, String> {
        let entry = &self.0;
        if matches!(*entry.status.borrow(), Status::Exited) {
            release(entry);
            return Ok(true);
        }
        if !matches!(*entry.status.borrow(), Status::Failed(_))
            || !entry
                .terminal_complete
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(false);
        }
        let mut runtime = match entry.runtime.try_lock() {
            Ok(runtime) => runtime,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
        };
        if entry.policy == StopPolicy::ContainedJob {
            return Err("Contained runtime requires a verified job join; root-only exit cannot release its fence; explicit retry required".into());
        }
        if let Some(agent) = runtime.as_mut() {
            if !poll_exited(agent)? {
                return Ok(false);
            }
        }
        drop(runtime.take());
        release(entry);
        entry.status.send_replace(Status::Exited);
        Ok(true)
    }
}

/// Cancellation can capture before the startup observer sees roster publication.
/// Match the retained incarnation even after exit cleared its runtime fields.
pub(crate) fn matching_pi_stop(
    home: &Path,
    agent_id: &str,
    generation: u64,
    status_arc: &Arc<Mutex<String>>,
) -> Result<Option<StopHandle>, String> {
    Ok(entries(&key(home, agent_id)?)
        .into_iter()
        .find(|entry| {
            entry.provider == "pi"
                && entry.generation == Some(generation)
                && Arc::ptr_eq(&entry.status_arc, status_arc)
        })
        .map(StopHandle))
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
            release(&entry);
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
    let spawned = std::thread::Builder::new().name("native-stop".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut runtime = worker.runtime.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(agent) = runtime.as_mut() {
                if !worker.identity_matches {
                    return Err("Captured runtime does not match reserved native identity; runtime retained".into());
                }
                if let Some(terminal) = &worker.terminal {
                    terminal.terminate(&worker.keys[0].1)?;
                }
                worker.terminal_complete.store(true, std::sync::atomic::Ordering::Release);
                if worker.owns_runtime {
                    match worker.policy {
                        StopPolicy::ContainedJob => stop_contained_runtime(agent)?,
                        StopPolicy::RetainedChild => stop_and_join(agent, worker.timeout, true)?,
                        StopPolicy::LegacyCodex => stop_and_join(agent, worker.timeout, false)?,
                    }
                }
            }
            // Terminal cleanup (when captured) and every child exit completed
            // before ActiveAgent Drop and release of the retained stop fence.
            drop(runtime.take());
            Ok(())
        })).unwrap_or_else(|_| Err("Codex stop worker panicked; runtime and fence retained".into()));
        match result {
            Ok(()) => {
                // Remove only this exact entry; a later capture is independent.
                // Pi startup may observe a cancelled registration after its
                // worker exits. Preserve that exact completion until observed.
                if worker.provider != "pi" {
                    release(&worker);
                }
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

/// Runs on the retained stop worker, so cancellation of the IPC waiter cannot
/// drop the runtime or admit a replacement. A failed join keeps the fence.
fn stop_contained_runtime(agent: &mut ActiveAgent) -> Result<(), String> {
    #[cfg(windows)]
    {
        if !agent.background_processes.is_empty() {
            return Err(
                "Contained runtime stop cannot verify background ownership; runtime retained"
                    .into(),
            );
        }
        let job = agent
            .job_object
            .as_ref()
            .filter(|job| job.contained_from_launch())
            .ok_or(
                "Contained runtime stop requires the retained prelaunch job; runtime retained",
            )?;
        let child = agent
            .child_process
            .as_mut()
            .ok_or("Contained runtime stop requires the retained child; runtime retained")?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|error| format!("Cannot create native stop executor: {error}"))?;
        executor.block_on(job.stop_and_join(child.as_mut()))?;
        // Only the verified job/member/child receipt permits these handles to drop.
        agent.child_process.take();
        agent.job_object.take();
        agent.process_id = None;
        agent.runtime_generation = None;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = agent;
        Err("Contained runtime stop is Windows-only; runtime retained".into())
    }
}

fn stop_and_join(
    agent: &mut ActiveAgent,
    timeout: Duration,
    retained_only: bool,
) -> Result<(), String> {
    // Preserve Pi's retained-child policy on retries as well as initial cleanup.
    #[cfg(not(windows))]
    let _ = retained_only;
    if poll_exited(agent)? {
        return Ok(());
    }
    let mut failure = None;
    if let Some(child) = agent.child_process.as_mut() {
        #[cfg(windows)]
        if let Some(pid) = child.process_id().filter(|_| !retained_only) {
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
        if !retained_only {
            if let Err(error) = crate::utils::process::force_kill_process_tree(child.id()) {
                failure = Some(error);
            }
        }
        if let Err(error) = child.kill() {
            failure = Some(error.to_string());
        }
    }
    #[cfg(windows)]
    {
        if !retained_only {
            agent.job_object.take();
        }
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
    // Every exit observer must reject a missing retained TUI handle. Empty
    // fields alone cannot prove a runtime with a live identity has exited.
    if agent.child_process.is_none()
        && (agent.process_id.is_some() || agent.runtime_generation.is_some())
    {
        return Err(
            "Cannot prove provider TUI exit: captured child handle is missing; fence retained"
                .into(),
        );
    }
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

/// Own the spawned child and prelaunch job before any terminal setup await.
/// This startup guard and PendingRuntime hand off to the same stop registry.
pub(crate) struct StartingRuntime {
    runtime: Option<ActiveAgent>,
    registration: Option<StopRegistration>,
    started: bool,
}

pub(crate) struct StartedProcess {
    pub(crate) child: Box<dyn portable_pty::Child + Send>,
    #[cfg(windows)]
    pub(crate) job: Option<crate::utils::process::RuntimeProcessJob>,
}

impl StartingRuntime {
    pub(crate) fn prepare(
        home: &Path,
        config: &wardian_core::models::AgentConfig,
        terminal: Arc<TerminalSessionBroker>,
    ) -> Result<Self, String> {
        let registration = if cfg!(windows) || matches!(config.provider.as_str(), "codex" | "pi") {
            Some(
                prepare_runtime_stop(home, &config.session_id, &config.provider)?
                    .with_terminal_cleanup(terminal)?,
            )
        } else {
            None
        };
        let runtime = ActiveAgent {
            config: Arc::new(Mutex::new(config.clone())),
            child_process: None,
            background_processes: Vec::new(),
            memory_capability: None,
            runtime_generation: None,
            process_id: None,
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            last_query_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Starting".into())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
                config.session_id.clone(),
                4096,
                262_144,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        };
        Ok(Self {
            runtime: Some(runtime),
            registration,
            started: false,
        })
    }

    /// Infallible handoff in the expression receiving the successful spawn.
    pub(crate) fn attach(
        mut self,
        child: Box<dyn portable_pty::Child + Send>,
        #[cfg(windows)] job: Option<crate::utils::process::RuntimeProcessJob>,
    ) -> Self {
        let runtime = self.runtime.as_mut().expect("prepared startup runtime");
        runtime.process_id = child.process_id();
        runtime.child_process = Some(child);
        #[cfg(windows)]
        {
            runtime.job_object = job;
        }
        self.started = true;
        self
    }

    pub(crate) fn child(&mut self) -> &mut dyn portable_pty::Child {
        self.runtime
            .as_mut()
            .unwrap()
            .child_process
            .as_mut()
            .expect("starting child")
            .as_mut()
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.runtime.as_ref().and_then(|runtime| runtime.process_id)
    }

    pub(crate) fn terminal_generation(&mut self, generation: u64) {
        self.runtime.as_mut().unwrap().runtime_generation = Some(generation);
    }

    pub(crate) fn finish(mut self) -> StartedProcess {
        self.started = false;
        self.registration.take();
        let runtime = self.runtime.as_mut().unwrap();
        runtime.process_id = None;
        runtime.runtime_generation = None;
        StartedProcess {
            child: runtime
                .child_process
                .take()
                .expect("starting child retained until attachment"),
            #[cfg(windows)]
            job: runtime.job_object.take(),
        }
    }

    pub(crate) fn begin_stop(mut self) -> Option<StopHandle> {
        self.stop()
    }

    fn stop(&mut self) -> Option<StopHandle> {
        if !std::mem::take(&mut self.started) {
            return None;
        }
        let mut runtime = self.runtime.take().unwrap();
        if let Some(registration) = self.registration.take() {
            Some(registration.capture(runtime).begin_stop())
        } else {
            // Preserve ordinary uncontained/non-Windows preattachment Drop.
            drop(runtime.child_process.take());
            runtime.process_id = None;
            runtime.runtime_generation = None;
            None
        }
    }
}

impl Drop for StartingRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
