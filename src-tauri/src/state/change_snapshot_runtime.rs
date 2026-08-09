//! Serializes and coalesces change snapshots per workspace.
//!
//! Snapshots run asynchronously and off the agent's critical path. Asynchrony is
//! what guards the agent, not latency, so this runtime never blocks a turn
//! boundary: it either starts a run or folds the request into one already in
//! flight.
//!
//! **Coalescing, not queueing.** If a snapshot is running when further turn
//! boundaries arrive, the workspace records only the most recent request.
//! Intermediate boundaries are dropped and the resulting snapshot is attributed
//! to the most recent effective turn. Queueing would let a burst of fast turns
//! accumulate unbounded work whose results are all superseded anyway.
//!
//! The lock is **per workspace**, matching the index. A working tree is one
//! filesystem, so two agents sharing a workspace produce one snapshot of the one
//! true tree rather than two competing views of it.

use crate::commands::change_snapshot::{take_snapshot, SnapshotOutcome, SnapshotRequest};
use crate::utils::logging::log_debug;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Injection seam so coalescing can be tested without spawning git.
pub type SnapshotFn =
    Arc<dyn Fn(&SnapshotRequest) -> Result<SnapshotOutcome, String> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDispatch {
    /// This call ran the snapshot loop; the value is how many snapshots it took.
    Ran(usize),
    /// A run was already in flight, so this request replaced the pending one.
    Coalesced,
}

#[derive(Default)]
struct WorkspaceSlot {
    in_flight: bool,
    pending: Option<SnapshotRequest>,
}

pub struct ChangeSnapshotRuntime {
    slots: Mutex<HashMap<String, WorkspaceSlot>>,
    snapshot_fn: SnapshotFn,
}

impl Default for ChangeSnapshotRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ChangeSnapshotRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChangeSnapshotRuntime")
            .finish_non_exhaustive()
    }
}

impl ChangeSnapshotRuntime {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            snapshot_fn: Arc::new(take_snapshot),
        }
    }

    #[cfg(test)]
    fn with_snapshot_fn(snapshot_fn: SnapshotFn) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            snapshot_fn,
        }
    }

    /// Runs a snapshot for the workspace, or folds the request into a run
    /// already in flight.
    ///
    /// The returned future completes when this call's own work is done. A
    /// coalesced request returns immediately; its snapshot is taken by whichever
    /// call currently owns the workspace.
    pub async fn snapshot(&self, request: SnapshotRequest) -> SnapshotDispatch {
        let workspace = request.cwd.clone();

        {
            let mut slots = self.slots.lock().await;
            let slot = slots.entry(workspace.clone()).or_default();
            if slot.in_flight {
                // Most recent wins. The superseded requests describe turns whose
                // content this snapshot will capture anyway.
                slot.pending = Some(request);
                return SnapshotDispatch::Coalesced;
            }
            slot.in_flight = true;
        }

        let mut next = Some(request);
        let mut taken = 0usize;
        while let Some(current) = next {
            let snapshot_fn = Arc::clone(&self.snapshot_fn);
            // Snapshots spawn git, so they never run on the async reactor.
            let result = tokio::task::spawn_blocking(move || snapshot_fn(&current))
                .await
                .unwrap_or_else(|error| Err(format!("snapshot task failed: {}", error)));
            taken += 1;

            if let Err(error) = result {
                // A snapshot failure is logged and dropped. Phase 2 never turns a
                // snapshot failure into a pane failure.
                log_debug(&format!(
                    "[change_snapshot] snapshot failed for {}: {}",
                    workspace, error
                ));
            }

            let mut slots = self.slots.lock().await;
            let slot = slots.entry(workspace.clone()).or_default();
            next = slot.pending.take();
            if next.is_none() {
                slot.in_flight = false;
            }
        }

        SnapshotDispatch::Ran(taken)
    }

    #[cfg(test)]
    async fn is_idle(&self, workspace: &str) -> bool {
        let slots = self.slots.lock().await;
        slots
            .get(workspace)
            .map(|slot| !slot.in_flight && slot.pending.is_none())
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::change_snapshot::ChangeSnapshotRef;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    fn request(workspace: &str, turn_index: u64) -> SnapshotRequest {
        SnapshotRequest {
            cwd: workspace.to_string(),
            agent_id: "agent-1".to_string(),
            conversation_id: "conv-1".to_string(),
            turn_index,
        }
    }

    fn created(turn_index: u64) -> SnapshotOutcome {
        SnapshotOutcome::Created(Box::new(ChangeSnapshotRef {
            agent_id: "agent-1".to_string(),
            conversation_id: "conv-1".to_string(),
            turn_index,
            commit_id: format!("commit-{turn_index}"),
            tree_id: format!("tree-{turn_index}"),
            created_at: "2026-08-02T00:00:00Z".to_string(),
        }))
    }

    #[tokio::test]
    async fn three_boundaries_during_one_in_flight_snapshot_yield_exactly_two_snapshots() {
        let observed: Arc<StdMutex<Vec<u64>>> = Arc::new(StdMutex::new(Vec::new()));
        let gate = Arc::new(tokio::sync::Notify::new());
        let entered = Arc::new(tokio::sync::Notify::new());

        let observed_fn = Arc::clone(&observed);
        let gate_fn = Arc::clone(&gate);
        let entered_fn = Arc::clone(&entered);
        let first_call = Arc::new(AtomicUsize::new(0));
        let runtime = Arc::new(ChangeSnapshotRuntime::with_snapshot_fn(Arc::new(
            move |request: &SnapshotRequest| {
                observed_fn.lock().unwrap().push(request.turn_index);
                // Hold only the first snapshot open, so the later boundaries all
                // arrive while it is still running.
                if first_call.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered_fn.notify_one();
                    let gate = Arc::clone(&gate_fn);
                    tokio::task::block_in_place(|| tauri::async_runtime::block_on(gate.notified()));
                }
                Ok(created(request.turn_index))
            },
        )));

        let runner = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move { runtime.snapshot(request("/w", 1)).await })
        };
        entered.notified().await;

        // Three further boundaries land while the first snapshot is in flight.
        for turn in 2..=4u64 {
            assert_eq!(
                runtime.snapshot(request("/w", turn)).await,
                SnapshotDispatch::Coalesced,
            );
        }

        gate.notify_one();
        let dispatch = runner.await.unwrap();

        assert_eq!(dispatch, SnapshotDispatch::Ran(2));
        let observed = observed.lock().unwrap().clone();
        assert_eq!(
            observed,
            vec![1, 4],
            "exactly two snapshots, the second attributed to the most recent turn",
        );
        assert!(runtime.is_idle("/w").await);
    }

    #[tokio::test]
    async fn sequential_requests_each_run() {
        let observed: Arc<StdMutex<Vec<u64>>> = Arc::new(StdMutex::new(Vec::new()));
        let observed_fn = Arc::clone(&observed);
        let runtime =
            ChangeSnapshotRuntime::with_snapshot_fn(Arc::new(move |request: &SnapshotRequest| {
                observed_fn.lock().unwrap().push(request.turn_index);
                Ok(created(request.turn_index))
            }));

        for turn in 1..=3u64 {
            assert_eq!(
                runtime.snapshot(request("/w", turn)).await,
                SnapshotDispatch::Ran(1)
            );
        }

        assert_eq!(*observed.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn separate_workspaces_do_not_block_each_other() {
        let observed: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let observed_fn = Arc::clone(&observed);
        let runtime =
            ChangeSnapshotRuntime::with_snapshot_fn(Arc::new(move |request: &SnapshotRequest| {
                observed_fn.lock().unwrap().push(request.cwd.clone());
                Ok(created(request.turn_index))
            }));

        assert_eq!(
            runtime.snapshot(request("/a", 1)).await,
            SnapshotDispatch::Ran(1)
        );
        assert_eq!(
            runtime.snapshot(request("/b", 1)).await,
            SnapshotDispatch::Ran(1)
        );

        assert_eq!(
            *observed.lock().unwrap(),
            vec!["/a".to_string(), "/b".to_string()]
        );
        assert!(runtime.is_idle("/a").await);
        assert!(runtime.is_idle("/b").await);
    }

    #[tokio::test]
    async fn a_failing_snapshot_releases_the_workspace() {
        let runtime = ChangeSnapshotRuntime::with_snapshot_fn(Arc::new(|_request| {
            Err("git exploded".to_string())
        }));

        assert_eq!(
            runtime.snapshot(request("/w", 1)).await,
            SnapshotDispatch::Ran(1)
        );

        // A failure must not wedge the workspace into a permanently in-flight
        // state, which would silently stop every later snapshot.
        assert!(runtime.is_idle("/w").await);
        assert_eq!(
            runtime.snapshot(request("/w", 2)).await,
            SnapshotDispatch::Ran(1)
        );
    }
}
