//! Coalesces latest-completion polling without queueing one archive read per
//! boundary. A claim owns the entire read/persist/emit round, including awaits.
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Runtime {
    session: String,
    status: Arc<Mutex<String>>,
}

impl PartialEq for Runtime {
    fn eq(&self, other: &Self) -> bool {
        self.session == other.session && Arc::ptr_eq(&self.status, &other.status)
    }
}

impl Eq for Runtime {}

impl Hash for Runtime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.session.hash(state);
        std::ptr::hash(Arc::as_ptr(&self.status), state);
    }
}

struct Entry {
    token: Arc<()>,
    pending: bool,
}

#[derive(Clone, Default)]
pub(crate) struct TranscriptCompletionCoordinator {
    entries: Arc<Mutex<HashMap<Runtime, Entry>>>,
}

pub(crate) struct CompletionClaim {
    coordinator: TranscriptCompletionCoordinator,
    runtime: Runtime,
    token: Arc<()>,
}

impl TranscriptCompletionCoordinator {
    /// Admit before spawning or touching the transcript. Distinct status cells
    /// have independent slots, so a late old notification cannot replace a new
    /// incarnation. Runtime authority is checked separately by the worker.
    pub(crate) fn notify(
        &self,
        session: &str,
        status: &Arc<Mutex<String>>,
    ) -> Option<CompletionClaim> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let runtime = Runtime {
            session: session.into(),
            status: status.clone(),
        };
        if let Some(entry) = entries.get_mut(&runtime) {
            entry.pending = true;
            return None;
        }
        let token = Arc::new(());
        entries.insert(
            runtime.clone(),
            Entry {
                token: token.clone(),
                pending: true,
            },
        );
        Some(CompletionClaim {
            coordinator: self.clone(),
            runtime,
            token,
        })
    }
}

impl CompletionClaim {
    fn next_round(&self) -> bool {
        let mut entries = self
            .coordinator
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = entries.get_mut(&self.runtime) else {
            return false;
        };
        if !Arc::ptr_eq(&entry.token, &self.token) {
            return false;
        }
        if entry.pending {
            entry.pending = false;
            true
        } else {
            // Notification admission uses this same lock. A signal arriving
            // after release claims a new worker instead of being cleared here.
            entries.remove(&self.runtime);
            false
        }
    }

    /// A completed or exhausted round consumes no later notification. No
    /// transcript work is detached, so cancellation drops it before ownership.
    pub(crate) async fn run<F, R>(self, mut round: F)
    where
        F: FnMut() -> R,
        R: Future<Output = ()>,
    {
        while self.next_round() {
            round().await;
        }
    }
}

impl Drop for CompletionClaim {
    fn drop(&mut self) {
        let mut entries = self
            .coordinator
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if entries
            .get(&self.runtime)
            .is_some_and(|entry| Arc::ptr_eq(&entry.token, &self.token))
        {
            entries.remove(&self.runtime);
        }
        // Cancellation grants no delivery guarantee. A later notification can
        // always claim again, even for this same status cell.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn burst_during_read_and_persistence_has_one_worker_and_latest_follow_up() {
        let coordinator = TranscriptCompletionCoordinator::default();
        let status = Arc::new(Mutex::new("Idle".into()));
        let claim = coordinator.notify("agent", &status).unwrap();
        let read_entered = Arc::new(Semaphore::new(0));
        let read_release = Arc::new(Semaphore::new(0));
        let persistence_entered = Arc::new(Semaphore::new(0));
        let persistence_release = Arc::new(Semaphore::new(0));
        let rounds = Arc::new(AtomicUsize::new(0));
        let worker = {
            let (read_entered, read_release, persistence_entered, persistence_release, rounds) = (
                read_entered.clone(),
                read_release.clone(),
                persistence_entered.clone(),
                persistence_release.clone(),
                rounds.clone(),
            );
            tokio::spawn(
                claim.run(move || {
                    let (
                        read_entered,
                        read_release,
                        persistence_entered,
                        persistence_release,
                        rounds,
                    ) = (
                        read_entered.clone(),
                        read_release.clone(),
                        persistence_entered.clone(),
                        persistence_release.clone(),
                        rounds.clone(),
                    );
                    async move {
                        if rounds.fetch_add(1, Ordering::SeqCst) == 0 {
                            read_entered.add_permits(1);
                            read_release.acquire().await.unwrap().forget();
                            persistence_entered.add_permits(1);
                            persistence_release.acquire().await.unwrap().forget();
                        }
                    }
                }),
            )
        };
        read_entered.acquire().await.unwrap().forget();
        for _ in 0..512 {
            assert!(coordinator.notify("agent", &status).is_none());
        }
        assert_eq!(rounds.load(Ordering::SeqCst), 1);
        read_release.add_permits(1);
        persistence_entered.acquire().await.unwrap().forget();
        assert!(coordinator.notify("agent", &status).is_none());
        persistence_release.add_permits(1);
        worker.await.unwrap();
        assert_eq!(rounds.load(Ordering::SeqCst), 2);
        assert!(
            coordinator.notify("agent", &status).is_some(),
            "a post-release boundary owns new work"
        );
    }

    #[test]
    fn release_and_successor_admission_are_atomic_and_old_cleanup_is_exact() {
        let coordinator = TranscriptCompletionCoordinator::default();
        let status = Arc::new(Mutex::new("Idle".into()));
        let old = coordinator.notify("agent", &status).unwrap();
        assert!(old.next_round());
        assert!(coordinator.notify("agent", &status).is_none());
        assert!(old.next_round(), "the signal before release remains owned");
        assert!(!old.next_round());
        let successor = coordinator.notify("agent", &status).unwrap();
        drop(old);
        assert!(coordinator.notify("agent", &status).is_none());
        assert!(successor.next_round());
        assert!(!successor.next_round());
    }

    #[tokio::test]
    async fn cancellation_releases_only_its_runtime_and_does_not_detach_the_round() {
        let coordinator = TranscriptCompletionCoordinator::default();
        let old_status = Arc::new(Mutex::new("Idle".into()));
        drop(coordinator.notify("agent", &old_status).unwrap());
        let claim = coordinator.notify("agent", &old_status).unwrap();
        let entered = Arc::new(Semaphore::new(0));
        let alive = Arc::new(AtomicUsize::new(0));
        struct InFlight(Arc<AtomicUsize>);
        impl Drop for InFlight {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let worker = {
            let (entered, alive) = (entered.clone(), alive.clone());
            tokio::spawn(claim.run(move || {
                let (entered, alive) = (entered.clone(), alive.clone());
                async move {
                    alive.fetch_add(1, Ordering::SeqCst);
                    let _in_flight = InFlight(alive);
                    entered.add_permits(1);
                    std::future::pending::<()>().await;
                }
            }))
        };
        entered.acquire().await.unwrap().forget();
        let new_status = Arc::new(Mutex::new("Idle".into()));
        let new_claim = coordinator.notify("agent", &new_status).unwrap();
        assert!(coordinator.notify("agent", &new_status).is_none());
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        assert_eq!(alive.load(Ordering::SeqCst), 0);
        assert!(coordinator.notify("agent", &new_status).is_none());
        let late_old = coordinator.notify("agent", &old_status).unwrap();
        drop(late_old);
        assert!(coordinator.notify("agent", &new_status).is_none());
        drop(new_claim);
    }

    #[tokio::test]
    async fn exhausted_round_stops_without_a_signal_and_pending_errors_retry_once() {
        let coordinator = TranscriptCompletionCoordinator::default();
        let status = Arc::new(Mutex::new("Idle".into()));
        let claim = coordinator.notify("agent", &status).unwrap();
        let rounds = AtomicUsize::new(0);
        claim
            .run(|| async {
                if rounds.fetch_add(1, Ordering::SeqCst) == 0 {
                    assert!(coordinator.notify("agent", &status).is_none());
                }
                // Completion or exhaustion both finish a round; neither may clear
                // the later signal, nor trigger an unlimited retry by themselves.
            })
            .await;
        assert_eq!(rounds.load(Ordering::SeqCst), 2);
        assert!(coordinator.notify("agent", &status).is_some());
    }
}
