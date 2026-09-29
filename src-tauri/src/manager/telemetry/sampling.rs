//! Bounds how long a status pass waits for the operating system's process table.

use super::{refresh_system_process_snapshot, SystemProcessSnapshot};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

/// The longest a pass waits for a process sample.
///
/// The refresh normally takes well under 300 ms. On Windows it reads every
/// process's command line and environment block when marker discovery is due,
/// and under load that has taken over a minute (observed: 72 s). Log-derived
/// status, liveness, and the observations the tick publishes all wait behind
/// it, so an overrun must not hold the pass.
const PROCESS_SAMPLE_BUDGET: Duration = Duration::from_secs(3);

/// Runs `sample` on its own thread and returns its result if it arrives within
/// `budget`.
///
/// On overrun this returns `None` and the thread keeps running. The pass then
/// proceeds exactly as it does when a previous refresh is still holding the
/// process table: liveness is unknown and no status is forced. Whatever the
/// slow refresh stores in the shared caches is there for the passes after it.
fn sample_within_budget<T: Send + 'static>(
    budget: Duration,
    sample: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("wardian-process-sample".to_string())
        .spawn(move || {
            // The receiver is gone after an overrun; the result is not needed.
            let _ = sender.send(sample());
        });
    if let Err(error) = spawned {
        crate::utils::logging::log_debug(&format!(
            "[Wardian] Telemetry skipped system sampling because its thread did not start: {error}"
        ));
        return None;
    }
    match receiver.recv_timeout(budget) {
        Ok(sample) => sample,
        Err(RecvTimeoutError::Timeout) => {
            crate::utils::logging::log_debug(&format!(
                "[Wardian] Telemetry continued without a process sample after {}s; the refresh is still running",
                budget.as_secs()
            ));
            None
        }
        Err(RecvTimeoutError::Disconnected) => None,
    }
}

/// [`refresh_system_process_snapshot`] under [`PROCESS_SAMPLE_BUDGET`].
pub(super) fn sample_processes(
    sys_metrics: Arc<tokio::sync::Mutex<sysinfo::System>>,
    session_ids: Vec<String>,
    agent_roots: Vec<(String, Option<u32>)>,
) -> Option<SystemProcessSnapshot> {
    sample_within_budget(PROCESS_SAMPLE_BUDGET, move || {
        refresh_system_process_snapshot(&sys_metrics, &session_ids, &agent_roots)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_sample_within_budget_is_returned() {
        let sample = sample_within_budget(Duration::from_secs(5), || Some(7));

        assert_eq!(sample, Some(7));
    }

    #[test]
    fn a_sample_that_overruns_returns_none_without_waiting_for_it() {
        let finished = Arc::new(AtomicBool::new(false));
        let signal = finished.clone();
        let started = std::time::Instant::now();

        let sample = sample_within_budget(Duration::from_millis(50), move || {
            std::thread::sleep(Duration::from_millis(600));
            signal.store(true, Ordering::SeqCst);
            Some(7)
        });

        assert_eq!(sample, None);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the pass must not wait out the slow sample"
        );
        // The slow refresh still completes in the background, so whatever it
        // stores in the shared caches reaches later passes.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !finished.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "sample never finished"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_sample_that_panics_yields_no_sample_instead_of_failing_the_pass() {
        let sample: Option<u32> =
            sample_within_budget(Duration::from_secs(5), || panic!("refresh failed"));

        assert_eq!(sample, None);
    }
}
