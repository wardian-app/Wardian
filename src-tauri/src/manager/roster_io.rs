use crate::state::AppState;
use wardian_core::{agent_replacement::AgentRosterBarrier, models::AgentConfig};

const ROSTER_BARRIER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

/// Poll durable exclusion without occupying a blocking worker while an async
/// owner may need that same pool to finish its physical write.
pub(crate) async fn acquire_roster_barrier() -> Result<AgentRosterBarrier, String> {
    acquire_roster_barrier_before(None).await
}

pub(crate) async fn acquire_roster_barrier_with_timeout(
    timeout: std::time::Duration,
) -> Result<AgentRosterBarrier, String> {
    acquire_roster_barrier_before(Some(tokio::time::Instant::now() + timeout)).await
}

async fn acquire_roster_barrier_before(
    deadline: Option<tokio::time::Instant>,
) -> Result<AgentRosterBarrier, String> {
    #[cfg(test)]
    let mut attempt = ROSTER_BARRIER_ATTEMPT
        .try_with(|signal| signal.borrow_mut().take())
        .ok()
        .flatten();
    loop {
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err("Timed out waiting for the agent roster barrier".to_string());
        }
        let acquire = tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
        });
        let acquired = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, acquire)
                .await
                .map_err(|_| "Timed out waiting for the agent roster barrier".to_string())?,
            None => acquire.await,
        }
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        if let Some(barrier) = acquired {
            if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                drop(barrier);
                return Err("Timed out waiting for the agent roster barrier".to_string());
            }
            return Ok(barrier);
        }
        #[cfg(test)]
        if let Some(signal) = attempt.take() {
            let _ = signal.send(());
        }
        match deadline {
            Some(deadline) => {
                tokio::time::timeout_at(deadline, tokio::time::sleep(ROSTER_BARRIER_RETRY_DELAY))
                    .await
                    .map_err(|_| "Timed out waiting for the agent roster barrier".to_string())?;
            }
            None => tokio::time::sleep(ROSTER_BARRIER_RETRY_DELAY).await,
        }
    }
}

#[cfg(test)]
tokio::task_local! {
    pub(crate) static ROSTER_BARRIER_ATTEMPT: std::cell::RefCell<Option<tokio::sync::oneshot::Sender<()>>>;
}

/// The physical operation owns durable exclusion and any lifecycle context
/// captured by its closure, even after the awaiting caller is cancelled.
pub(crate) async fn run_roster_io<T: Send + 'static>(
    barrier: AgentRosterBarrier,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    #[cfg(test)]
    let probe = super::ROSTER_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    let operation = move || {
        let _barrier = barrier;
        operation()
    };
    #[cfg(test)]
    let operation =
        move || super::ROSTER_IO_PROBE.sync_scope(std::cell::RefCell::new(probe), operation);
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("Roster I/O task failed: {error}"))?
}

/// Preserve the best-effort snapshot-write policy while the worker owns the
/// caller's lifecycle context. Snapshot capture must already be under barrier.
pub(crate) async fn save_snapshot<C: Send + 'static>(
    barrier: AgentRosterBarrier,
    configs: Vec<AgentConfig>,
    context: C,
) -> Result<C, String> {
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        super::log_debug(
            "[WARDIAN] Failed to persist state snapshot: Could not locate Wardian home",
        );
        return Ok(context);
    };
    run_roster_io(barrier, move || {
        if let Err(error) = super::try_save_state_snapshot_for_home(&home, &configs) {
            super::log_debug(&format!(
                "[WARDIAN] Failed to persist state snapshot: {error}"
            ));
        }
        Ok(context)
    })
    .await
}

/// Capture current live state only after durable admission. A caller must not
/// pass a snapshot taken before waiting for the barrier.
pub(crate) async fn save_live_state<C: Send + 'static>(
    state: &AppState,
    context: C,
) -> Result<C, String> {
    let barrier = match acquire_roster_barrier().await {
        Ok(barrier) => barrier,
        Err(error) => {
            super::log_debug(&format!(
                "[WARDIAN] Failed to persist state snapshot: {error}"
            ));
            return Ok(context);
        }
    };
    let configs = {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        super::state_configs_snapshot(&agents, &order)
    };
    save_snapshot(barrier, configs, context).await
}

/// Strict persistence for an owned continuation that retains admission through
/// filesystem publication, SQLite deletion and live publication or compensation.
/// Unlike best-effort saves, deletion must observe the physical write's error.
pub(crate) async fn write_snapshot_strict(
    _barrier: &AgentRosterBarrier,
    home: std::path::PathBuf,
    configs: Vec<AgentConfig>,
) -> Result<(), String> {
    #[cfg(test)]
    let probe = super::ROSTER_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    let operation = move || super::try_save_state_snapshot_for_home(&home, &configs);
    #[cfg(test)]
    let operation =
        move || super::ROSTER_IO_PROBE.sync_scope(std::cell::RefCell::new(probe), operation);
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("Roster deletion I/O task failed: {error}"))?
}
