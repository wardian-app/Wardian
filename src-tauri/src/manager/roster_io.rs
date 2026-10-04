use crate::state::AppState;
use wardian_core::{agent_replacement::AgentRosterBarrier, models::AgentConfig};

/// Poll durable exclusion without occupying a blocking worker while an async
/// owner may need that same pool to finish its physical write.
pub(crate) async fn acquire_roster_barrier() -> Result<AgentRosterBarrier, String> {
    #[cfg(test)]
    let mut attempt = ROSTER_BARRIER_ATTEMPT
        .try_with(|signal| signal.borrow_mut().take())
        .ok()
        .flatten();
    loop {
        let barrier = tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        if let Some(barrier) = barrier {
            return Ok(barrier);
        }
        #[cfg(test)]
        if let Some(signal) = attempt.take() {
            let _ = signal.send(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
