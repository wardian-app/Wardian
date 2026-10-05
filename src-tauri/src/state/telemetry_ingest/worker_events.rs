//! Focused filesystem notifications for Codex child rollout discovery.
//!
//! This helper only reports candidate paths. The owner remains responsible for
//! bounded metadata reads, ancestry verification, and worker registration.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use notify::Watcher as _;
use tokio::sync::Notify;

const MAX_PENDING_PATHS: usize = 4096;
const MAX_EVENT_PATHS_PER_CALLBACK: usize = 128;
const MAX_SESSION_ROOTS: usize = 4096;

type SharedPendingPaths = Arc<Mutex<PendingPaths>>;

struct PendingPaths {
    capacity: usize,
    paths: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    overflowed: bool,
    wake: WorkerEventWake,
}

impl PendingPaths {
    fn new(capacity: usize) -> Self {
        Self::with_wake(capacity, WorkerEventWake::new())
    }

    fn with_wake(capacity: usize, wake: WorkerEventWake) -> Self {
        Self {
            capacity: capacity.clamp(1, MAX_PENDING_PATHS),
            paths: VecDeque::new(),
            queued: HashSet::new(),
            overflowed: false,
            wake,
        }
    }

    fn enqueue(&mut self, path: PathBuf) {
        if self.wake.is_closed() {
            return;
        }
        if self.queued.contains(&path) {
            return;
        }
        if self.paths.len() >= self.capacity {
            self.overflowed = true;
            self.wake.signal();
            return;
        }
        self.queued.insert(path.clone());
        self.paths.push_back(path);
        self.wake.signal();
    }

    fn drain(&mut self, limit: usize) -> WorkerEventBatch {
        let limit = limit.min(self.capacity);
        let mut paths = Vec::with_capacity(limit.min(self.paths.len()));
        for _ in 0..limit {
            let Some(path) = self.paths.pop_front() else {
                break;
            };
            self.queued.remove(&path);
            paths.push(path);
        }
        if !self.paths.is_empty() {
            self.wake.signal();
        }
        WorkerEventBatch {
            paths,
            overflowed: std::mem::take(&mut self.overflowed),
        }
    }

    fn mark_overflowed(&mut self) {
        self.overflowed = true;
        self.wake.signal();
    }

    fn mark_closed(&mut self) {
        self.wake.close();
    }
}

/// Coalesced async wake and terminal state for the bounded worker-event queue.
/// The queue remains the source of truth; callers should drain it after waking
/// and use periodic reconciliation whenever a batch reports overflow.
#[derive(Clone)]
pub(super) struct WorkerEventWake {
    notify: Arc<Notify>,
    closed: Arc<AtomicBool>,
}

impl WorkerEventWake {
    fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn signal(&self) {
        self.notify.notify_one();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Wait for a coalesced event signal. Returns `false` once the watcher is
    /// permanently closed. Registering before checking closure preserves the
    /// close wake if shutdown races with the caller entering this wait.
    pub(super) async fn notified(&self) -> bool {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_closed() {
            return false;
        }
        notified.await;
        !self.is_closed()
    }
}

/// A bounded batch of candidate rollout paths and a recovery signal.
#[derive(Debug)]
pub(super) struct WorkerEventBatch {
    /// Candidate hints only. Notify callbacks cannot identify the root epoch
    /// that produced a delayed event, so the owner must validate current
    /// canonical root ancestry and rollout identity before lifecycle publish.
    pub(super) paths: Vec<PathBuf>,
    /// True when a path, watcher operation, or notification may have been lost.
    /// The caller should rely on the existing periodic full reconciliation.
    pub(super) overflowed: bool,
}

/// Result of replacing the set of recursively watched Codex `sessions` roots.
#[derive(Debug)]
pub(super) struct SessionsRootUpdate {
    /// Number of requested canonical roots that are currently watched.
    pub(super) watched_roots: usize,
    /// Requested roots omitted because the bounded update limit was reached.
    pub(super) truncated_roots: usize,
    /// Roots that could not be canonicalized, watched, or unwatched.
    pub(super) failed_roots: Vec<PathBuf>,
}

enum WatcherCommand {
    UpdateRoots(Vec<PathBuf>, usize, mpsc::Sender<SessionsRootUpdate>),
    Shutdown,
}

/// Owns the notification thread and exposes its bounded path queue to the
/// telemetry worker. Dropping this value closes the command channel and joins
/// the owned thread.
pub(super) struct CodexWorkerEventWatcher {
    commands: Option<mpsc::SyncSender<WatcherCommand>>,
    pending: SharedPendingPaths,
    wake: WorkerEventWake,
    worker: Option<JoinHandle<()>>,
}

impl CodexWorkerEventWatcher {
    /// Start the owned watcher thread. Initialization failures are surfaced as
    /// an overflow and terminal wake so periodic reconciliation remains the
    /// recovery path.
    pub(super) fn new(queue_capacity: usize) -> Self {
        let wake = WorkerEventWake::new();
        let pending = Arc::new(Mutex::new(PendingPaths::with_wake(
            queue_capacity,
            wake.clone(),
        )));
        let (commands, receiver) = mpsc::sync_channel(1);
        let worker_pending = Arc::clone(&pending);
        let worker = thread::Builder::new()
            .name("codex-worker-events".to_owned())
            .spawn(move || run_watcher(receiver, worker_pending));

        match worker {
            Ok(worker) => Self {
                commands: Some(commands),
                pending,
                wake,
                worker: Some(worker),
            },
            Err(_) => {
                mark_overflow(&pending);
                mark_closed(&pending);
                Self {
                    commands: None,
                    pending,
                    wake,
                    worker: None,
                }
            }
        }
    }

    /// Replace watched session roots. At most one command can be queued, root
    /// sets are capped, and a busy worker rejects the update for periodic
    /// reconciliation. Canonicalization and watcher operations run on the
    /// owned worker, never in notify's callback.
    pub(super) fn update_sessions_roots(
        &self,
        roots: Vec<PathBuf>,
    ) -> Result<SessionsRootUpdate, WatcherUpdateError> {
        let Some(commands) = &self.commands else {
            mark_overflow(&self.pending);
            return Err(WatcherUpdateError::Closed);
        };
        let (roots, truncated_roots) = limit_session_roots(roots);
        if truncated_roots > 0 {
            mark_overflow(&self.pending);
        }
        let (reply, result) = mpsc::channel();
        match commands.try_send(WatcherCommand::UpdateRoots(roots, truncated_roots, reply)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                mark_overflow(&self.pending);
                return Err(WatcherUpdateError::Busy);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                mark_overflow(&self.pending);
                mark_closed(&self.pending);
                return Err(WatcherUpdateError::Closed);
            }
        }
        result.recv().map_err(|_| {
            mark_overflow(&self.pending);
            mark_closed(&self.pending);
            WatcherUpdateError::Closed
        })
    }

    /// Clone the coalesced async wake and terminal signal for the single owner
    /// loop. Notifications are hints; they do not qualify a source for publish.
    pub(super) fn event_wake(&self) -> WorkerEventWake {
        self.wake.clone()
    }

    /// Drain at most `limit` queued paths and consume the current recovery
    /// signal. Both the configured queue and each returned batch are bounded.
    pub(super) fn drain(&self, limit: usize) -> WorkerEventBatch {
        lock_pending(&self.pending).drain(limit)
    }

    /// Stop and join the owned thread. Safe to call more than once.
    pub(super) fn close(&mut self) {
        if let Some(commands) = self.commands.take() {
            if commands.send(WatcherCommand::Shutdown).is_err() {
                mark_overflow(&self.pending);
                mark_closed(&self.pending);
            }
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                mark_overflow(&self.pending);
                mark_closed(&self.pending);
            }
        }
    }
}

impl Drop for CodexWorkerEventWatcher {
    fn drop(&mut self) {
        self.close();
    }
}

/// Why a root-set update could not be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WatcherUpdateError {
    /// A root update is already being processed or queued.
    Busy,
    /// The watcher thread has stopped or its channel is gone.
    Closed,
}

struct WatcherLifetime(SharedPendingPaths);

impl Drop for WatcherLifetime {
    fn drop(&mut self) {
        mark_closed(&self.0);
    }
}

fn run_watcher(commands: mpsc::Receiver<WatcherCommand>, pending: SharedPendingPaths) {
    let _lifetime = WatcherLifetime(Arc::clone(&pending));
    let callback_pending = Arc::clone(&pending);
    let mut watcher =
        match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            record_notify_result(result, &callback_pending)
        }) {
            Ok(watcher) => watcher,
            Err(_) => {
                mark_overflow(&pending);
                mark_closed(&pending);
                return;
            }
        };
    let mut watched_roots = HashSet::new();

    while let Ok(command) = commands.recv() {
        match command {
            WatcherCommand::UpdateRoots(roots, truncated_roots, reply) => {
                let result = replace_sessions_roots(
                    &mut watcher,
                    &mut watched_roots,
                    roots,
                    truncated_roots,
                    &pending,
                );
                let _ = reply.send(result);
            }
            WatcherCommand::Shutdown => break,
        }
    }
    drop(watcher);
}

fn replace_sessions_roots(
    watcher: &mut notify::RecommendedWatcher,
    watched_roots: &mut HashSet<PathBuf>,
    roots: Vec<PathBuf>,
    truncated_roots: usize,
    pending: &SharedPendingPaths,
) -> SessionsRootUpdate {
    let canonical = canonicalize_roots(roots);
    let desired: HashSet<_> = canonical.roots.iter().cloned().collect();
    let mut failed_roots = canonical.failed_roots;

    let removed: Vec<_> = watched_roots
        .iter()
        .filter(|root| !desired.contains(*root))
        .cloned()
        .collect();
    for root in removed {
        match watcher.unwatch(&root) {
            Ok(()) => {
                watched_roots.remove(&root);
            }
            Err(_) => failed_roots.push(root),
        }
    }

    for root in canonical.roots {
        if watched_roots.contains(&root) {
            continue;
        }
        match watcher.watch(&root, notify::RecursiveMode::Recursive) {
            Ok(()) => {
                watched_roots.insert(root);
            }
            Err(_) => failed_roots.push(root),
        }
    }

    failed_roots.sort();
    failed_roots.dedup();
    if !failed_roots.is_empty() || truncated_roots > 0 {
        mark_overflow(pending);
    }
    let watched_roots = desired
        .iter()
        .filter(|root| watched_roots.contains(*root))
        .count();
    SessionsRootUpdate {
        watched_roots,
        truncated_roots,
        failed_roots,
    }
}

fn limit_session_roots(roots: Vec<PathBuf>) -> (Vec<PathBuf>, usize) {
    let truncated_roots = roots.len().saturating_sub(MAX_SESSION_ROOTS);
    // Rebuild the capped list so a caller's oversized Vec allocation does not
    // remain attached to the queued root update.
    let mut bounded = Vec::with_capacity(roots.len().min(MAX_SESSION_ROOTS));
    bounded.extend(roots.into_iter().take(MAX_SESSION_ROOTS));
    (bounded, truncated_roots)
}

struct CanonicalRoots {
    roots: Vec<PathBuf>,
    failed_roots: Vec<PathBuf>,
}

fn canonicalize_roots(roots: Vec<PathBuf>) -> CanonicalRoots {
    let mut canonical_roots = Vec::new();
    let mut seen_roots = HashSet::new();
    let mut seen_inputs = HashSet::new();
    let mut failed_roots = Vec::new();

    for root in roots {
        if !seen_inputs.insert(root.clone()) {
            continue;
        }
        match std::fs::canonicalize(&root) {
            Ok(canonical) if seen_roots.insert(canonical.clone()) => {
                canonical_roots.push(canonical);
            }
            Ok(_) => {}
            Err(_) => failed_roots.push(root),
        }
    }
    CanonicalRoots {
        roots: canonical_roots,
        failed_roots,
    }
}

fn record_notify_result(result: notify::Result<notify::Event>, pending: &SharedPendingPaths) {
    let event = match result {
        Ok(event) => event,
        Err(_) => {
            mark_overflow(pending);
            return;
        }
    };
    if !matches!(
        &event.kind,
        notify::EventKind::Any | notify::EventKind::Create(_) | notify::EventKind::Modify(_)
    ) {
        return;
    }

    let too_many_paths = event.paths.len() > MAX_EVENT_PATHS_PER_CALLBACK;
    let candidates: Vec<_> = event
        .paths
        .into_iter()
        .take(MAX_EVENT_PATHS_PER_CALLBACK)
        .filter(|path| is_rollout_jsonl(path))
        .collect();
    let mut pending = lock_pending(pending);
    if too_many_paths {
        pending.mark_overflowed();
    }
    for path in candidates {
        pending.enqueue(path);
    }
}

fn is_rollout_jsonl(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.strip_prefix("rollout-")
        .and_then(|suffix| suffix.strip_suffix(".jsonl"))
        .is_some_and(|identity| !identity.is_empty())
}

fn mark_overflow(pending: &SharedPendingPaths) {
    lock_pending(pending).mark_overflowed();
}

fn mark_closed(pending: &SharedPendingPaths) {
    lock_pending(pending).mark_closed();
}

fn lock_pending(pending: &SharedPendingPaths) -> MutexGuard<'_, PendingPaths> {
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind};
    use notify::{Event, EventKind};

    #[test]
    fn queues_only_create_or_modify_events_for_rollout_jsonl_paths() {
        let pending = Arc::new(Mutex::new(PendingPaths::new(8)));
        let rollout = PathBuf::from("sessions/rollout-2026-10-04T12-00-00-thread.jsonl");
        let unrelated = PathBuf::from("sessions/history.jsonl");
        let mut create = Event::new(EventKind::Create(CreateKind::File));
        create.paths.push(rollout.clone());
        create.paths.push(unrelated);
        record_notify_result(Ok(create), &pending);

        let mut remove = Event::new(EventKind::Remove(notify::event::RemoveKind::File));
        remove
            .paths
            .push(PathBuf::from("sessions/rollout-old-thread.jsonl"));
        record_notify_result(Ok(remove), &pending);

        let batch = lock_pending(&pending).drain(8);
        assert_eq!(batch.paths, vec![rollout]);
        assert!(!batch.overflowed);

        let mut modify = Event::new(EventKind::Modify(ModifyKind::Any));
        modify.paths.push(PathBuf::from("sessions/rollout-.jsonl"));
        record_notify_result(Ok(modify), &pending);
        assert!(lock_pending(&pending).drain(8).paths.is_empty());
    }

    #[test]
    fn coalesces_duplicate_paths_until_they_are_drained() {
        let mut pending = PendingPaths::new(2);
        let rollout = PathBuf::from("sessions/rollout-thread.jsonl");
        pending.enqueue(rollout.clone());
        pending.enqueue(rollout.clone());

        let first = pending.drain(1);
        assert_eq!(first.paths, vec![rollout.clone()]);
        assert!(!first.overflowed);

        pending.enqueue(rollout.clone());
        assert_eq!(pending.drain(1).paths, vec![rollout]);
    }

    #[test]
    fn overflow_is_bounded_and_surfaces_periodic_recovery_signal() {
        let mut pending = PendingPaths::new(2);
        pending.enqueue(PathBuf::from("sessions/rollout-one.jsonl"));
        pending.enqueue(PathBuf::from("sessions/rollout-two.jsonl"));
        pending.enqueue(PathBuf::from("sessions/rollout-three.jsonl"));

        let first = pending.drain(1);
        assert_eq!(first.paths.len(), 1);
        assert!(first.overflowed);
        let second = pending.drain(8);
        assert_eq!(second.paths.len(), 1);
        assert!(!second.overflowed);
    }

    #[test]
    fn oversized_notify_event_is_capped_and_signals_recovery() {
        let pending = Arc::new(Mutex::new(PendingPaths::new(256)));
        let mut event = Event::new(EventKind::Modify(ModifyKind::Any));
        for index in 0..(MAX_EVENT_PATHS_PER_CALLBACK + 5) {
            event
                .paths
                .push(PathBuf::from(format!("sessions/rollout-{index}.jsonl")));
        }
        record_notify_result(Ok(event), &pending);

        let batch = lock_pending(&pending).drain(usize::MAX);
        assert_eq!(batch.paths.len(), MAX_EVENT_PATHS_PER_CALLBACK);
        assert!(batch.overflowed);
    }

    #[test]
    fn watcher_failures_surface_as_recovery_signals() {
        let pending = Arc::new(Mutex::new(PendingPaths::new(1)));
        mark_overflow(&pending);
        assert!(lock_pending(&pending).drain(1).overflowed);
    }

    #[test]
    fn canonicalizes_and_deduplicates_session_roots() {
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join(".");
        let canonical = canonicalize_roots(vec![root.path().to_path_buf(), alias]);

        assert!(canonical.failed_roots.is_empty());
        assert_eq!(canonical.roots.len(), 1);
        assert_eq!(
            canonical.roots[0],
            std::fs::canonicalize(root.path()).unwrap()
        );
    }

    #[test]
    fn missing_session_root_is_reported_for_periodic_recovery() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("not-created");
        let canonical = canonicalize_roots(vec![missing.clone()]);

        assert!(canonical.roots.is_empty());
        assert_eq!(canonical.failed_roots, vec![missing]);
    }

    #[test]
    fn session_root_updates_are_capped_before_queueing() {
        let mut roots = Vec::with_capacity(MAX_SESSION_ROOTS * 2);
        roots.extend(
            (0..(MAX_SESSION_ROOTS + 3)).map(|index| PathBuf::from(format!("sessions/{index}"))),
        );
        let (bounded, truncated) = limit_session_roots(roots);

        assert_eq!(bounded.len(), MAX_SESSION_ROOTS);
        assert!(bounded.capacity() <= MAX_SESSION_ROOTS);
        assert_eq!(truncated, 3);
    }

    #[tokio::test]
    async fn async_wake_signals_overflow_when_path_queue_is_full() {
        let mut pending = PendingPaths::new(1);
        let wake = pending.wake.clone();
        pending.enqueue(PathBuf::from("sessions/rollout-one.jsonl"));
        assert!(wake.notified().await);

        pending.enqueue(PathBuf::from("sessions/rollout-two.jsonl"));
        assert!(wake.notified().await);
        assert!(pending.drain(1).overflowed);
    }

    #[tokio::test]
    async fn async_wake_coalesces_events_and_rearms_for_remaining_paths() {
        let mut pending = PendingPaths::new(4);
        let wake = pending.wake.clone();
        pending.enqueue(PathBuf::from("sessions/rollout-one.jsonl"));
        pending.enqueue(PathBuf::from("sessions/rollout-two.jsonl"));

        assert!(wake.notified().await);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), wake.notified())
                .await
                .is_err()
        );

        let first = pending.drain(1);
        assert_eq!(first.paths.len(), 1);
        assert!(wake.notified().await);
        assert_eq!(pending.drain(4).paths.len(), 1);

        pending.enqueue(PathBuf::from("sessions/rollout-three.jsonl"));
        assert!(wake.notified().await);
    }

    #[tokio::test]
    async fn async_wake_cannot_lose_a_signal_while_the_waiter_arms() {
        let pending = Arc::new(Mutex::new(PendingPaths::new(2)));
        let wake = lock_pending(&pending).wake.clone();
        let waiting = tokio::spawn({
            let wake = wake.clone();
            async move { wake.notified().await }
        });

        tokio::task::yield_now().await;
        lock_pending(&pending).enqueue(PathBuf::from("sessions/rollout-race.jsonl"));

        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn async_wake_reports_terminal_lifetime_after_close() {
        let pending = Arc::new(Mutex::new(PendingPaths::new(2)));
        let wake = lock_pending(&pending).wake.clone();
        let waiting = tokio::spawn({
            let wake = wake.clone();
            async move { wake.notified().await }
        });

        tokio::task::yield_now().await;
        lock_pending(&pending).mark_closed();

        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .unwrap()
        );
        assert!(!wake.notified().await);
    }

    #[tokio::test]
    async fn close_joins_owned_worker_and_is_idempotent() {
        let mut watcher = CodexWorkerEventWatcher::new(4);
        let wake = watcher.event_wake();
        watcher.close();
        assert!(watcher.commands.is_none());
        assert!(watcher.worker.is_none());
        assert!(!wake.notified().await);
        watcher.close();
        assert!(watcher.commands.is_none());
        assert!(watcher.worker.is_none());
    }
}
