//! Orders conversation-capture policy work across the whole app.
//!
//! Every archive capture pass and every logging-policy transition takes one
//! exclusive gate. A plain FIFO mutex makes a lifecycle boundary such as
//! New Session wait behind every background capture that queued earlier: after
//! a restart each restored agent schedules its own sync, so the boundary could
//! sit in that queue for minutes while the UI showed nothing.
//!
//! Background captures therefore never park in the queue. They poll with
//! `try_lock` and stand aside while a lifecycle boundary is registered, so a
//! boundary waits for at most the pass already running.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard};

const BACKGROUND_RETRY: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct CapturePolicyGate {
    lock: Mutex<()>,
    lifecycle_boundaries: AtomicUsize,
}

/// Registration held for the whole span of a lifecycle boundary. Holding it
/// across consecutive passes keeps background captures from slipping in
/// between them.
pub struct LifecycleBoundary<'gate> {
    gate: &'gate CapturePolicyGate,
}

impl Drop for LifecycleBoundary<'_> {
    fn drop(&mut self) {
        self.gate
            .lifecycle_boundaries
            .fetch_sub(1, Ordering::SeqCst);
    }
}

impl CapturePolicyGate {
    /// Queues behind the running holder and any earlier queued caller. Policy
    /// transitions and lifecycle boundaries use this lane.
    pub async fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().await
    }

    pub fn try_lock(&self) -> Result<MutexGuard<'_, ()>, tokio::sync::TryLockError> {
        self.lock.try_lock()
    }

    /// Registers a lifecycle boundary until the returned value is dropped.
    pub fn begin_lifecycle_boundary(&self) -> LifecycleBoundary<'_> {
        self.lifecycle_boundaries.fetch_add(1, Ordering::SeqCst);
        LifecycleBoundary { gate: self }
    }

    /// Acquires the gate for best-effort capture. Yields to any registered
    /// lifecycle boundary and to any caller queued in [`Self::lock`].
    pub async fn lock_background(&self) -> MutexGuard<'_, ()> {
        loop {
            if self.lifecycle_boundaries.load(Ordering::SeqCst) == 0 {
                if let Ok(guard) = self.lock.try_lock() {
                    return guard;
                }
            }
            tokio::time::sleep(BACKGROUND_RETRY).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn lifecycle_boundary_overtakes_queued_background_captures() {
        let gate = Arc::new(CapturePolicyGate::default());
        let order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let running = gate.lock().await;

        let mut background = Vec::new();
        for index in 0..8 {
            let gate = gate.clone();
            let order = order.clone();
            background.push(tokio::spawn(async move {
                let _guard = gate.lock_background().await;
                order.lock().unwrap().push(format!("background-{index}"));
                tokio::time::sleep(Duration::from_millis(500)).await;
            }));
        }
        settle().await;

        let boundary = {
            let gate = gate.clone();
            let order = order.clone();
            tokio::spawn(async move {
                let _registration = gate.begin_lifecycle_boundary();
                let _guard = gate.lock().await;
                order.lock().unwrap().push("boundary".to_string());
            })
        };
        settle().await;

        drop(running);
        boundary.await.unwrap();
        for task in background {
            task.await.unwrap();
        }

        let order = order.lock().unwrap().clone();
        assert_eq!(order.len(), 9);
        assert_eq!(
            order[0], "boundary",
            "the lifecycle boundary must run before background captures queued ahead of it: {order:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn background_captures_stand_aside_for_the_whole_boundary() {
        let gate = CapturePolicyGate::default();
        let registration = gate.begin_lifecycle_boundary();

        let mut waiting = std::pin::pin!(gate.lock_background());
        tokio::select! {
            _ = &mut waiting => panic!("background capture ran during a lifecycle boundary"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        }

        drop(registration);
        let _guard = waiting.await;
    }

    #[tokio::test(start_paused = true)]
    async fn background_capture_does_not_jump_a_queued_policy_transition() {
        let gate = Arc::new(CapturePolicyGate::default());
        let running = gate.lock().await;
        let transition = {
            let gate = gate.clone();
            tokio::spawn(async move {
                let _guard = gate.lock().await;
            })
        };
        settle().await;

        drop(running);
        // The queued transition owns the next hand-off, so a poll cannot win.
        assert!(gate.try_lock().is_err());
        transition.await.unwrap();
        assert!(gate.try_lock().is_ok());
    }
}
