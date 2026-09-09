//! The roster publication boundary used by application startup restoration.

use crate::state::{ActiveAgent, AppState};
use std::sync::{Arc, Mutex};

/// Own one startup restoration from config selection through final publication.
pub(crate) struct RestorePublication {
    session_id: String,
    _lifecycle: tokio::sync::OwnedMutexGuard<()>,
}

impl RestorePublication {
    /// Claim an unregistered agent before selecting its saved configuration.
    /// A registered owner wins over the startup snapshot, including when a
    /// mutation completed while this claim waited for the lifecycle gate.
    /// Keep the claim alive through placeholder and final/error publication.
    pub(crate) async fn begin(state: &AppState, session_id: &str) -> Option<Self> {
        let lifecycle = state.lock_agent_lifecycle(session_id).await;
        if state.agents.lock().await.contains_key(session_id) {
            return None;
        }
        Some(Self {
            session_id: session_id.to_owned(),
            _lifecycle: lifecycle,
        })
    }

    /// Publish either the placeholder or its completed runtime without holding
    /// the roster locks during provider initialization.
    pub(crate) async fn publish(&self, state: &AppState, agent: ActiveAgent) -> Arc<Mutex<String>> {
        let session_id = agent.config.lock().unwrap().session_id.clone();
        assert_eq!(
            session_id, self.session_id,
            "restoration claim belongs to another agent"
        );
        let status = agent.current_status.clone();
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        if !order.contains(&session_id) {
            order.push(session_id.clone());
        }
        agents.insert(session_id, agent);
        status
    }
}

/// Persist the current roster once startup's publications have completed.
pub(crate) async fn persist_roster(state: &AppState) -> Result<(), String> {
    loop {
        {
            let agents = state.agents.lock().await;
            let order = state.agent_order.lock().await;
            // Config mutation can own the durable barrier while waiting for
            // these maps. Never wait for that barrier while holding the maps.
            if let Some(_barrier) =
                wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                    .map_err(|error| error.to_string())?
            {
                let snapshot = crate::manager::state_configs_snapshot(&agents, &order);
                return crate::manager::try_save_state_snapshot_unlocked(&snapshot);
            }
        }
        // Wait off the async executor, with no roster locks or stale snapshot.
        // Re-read the current roster after the competing writer completes.
        tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(true).map(drop)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
}

#[cfg(test)]
mod tests;
