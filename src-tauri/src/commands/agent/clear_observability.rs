//! Entry point for New Session that makes the operation observable.
//!
//! The clear takes a long, multi-step path (lease, archive drain, replacement
//! spawn). Two things used to go wrong silently: a failure left no trace in the
//! debug log, and a second request for the same agent queued behind the first
//! and then failed on the conversation lease. This wrapper records every
//! outcome and refuses a duplicate immediately.

use super::{clear_agent_session_inner, ClearAgentLifecycle};
use crate::manager;
use crate::state::AppState;
use tauri::{AppHandle, State};

/// Claim on an agent's New Session. Dropping it frees the agent for the next one.
struct ClearInFlight<'state> {
    state: &'state AppState,
    session_id: String,
    started: std::time::Instant,
}

impl<'state> ClearInFlight<'state> {
    fn begin(state: &'state AppState, session_id: &str) -> Result<Self, String> {
        let claimed = state
            .clears_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id.to_string());
        if !claimed {
            return Err("A new session is already starting for this agent.".to_string());
        }
        Ok(Self {
            state,
            session_id: session_id.to_string(),
            started: std::time::Instant::now(),
        })
    }

    fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }
}

impl Drop for ClearInFlight<'_> {
    fn drop(&mut self) {
        self.state
            .clears_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.session_id);
    }
}

pub(super) async fn run(
    session_id: String,
    reason: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let clear = match ClearInFlight::begin(state.inner(), &session_id) {
        Ok(clear) => clear,
        Err(error) => {
            manager::log_debug(&format!(
                "[WARDIAN] clear_agent_session refused for {session_id}: {error}"
            ));
            return Err(error);
        }
    };
    let result = clear_agent_session_inner(
        session_id.clone(),
        reason,
        state,
        app,
        None,
        ClearAgentLifecycle::default(),
    )
    .await;
    match &result {
        Ok(()) => manager::log_debug(&format!(
            "[WARDIAN] clear_agent_session finished for {session_id} in {} ms",
            clear.elapsed_ms()
        )),
        Err(error) => manager::log_debug(&format!(
            "[WARDIAN] clear_agent_session failed for {session_id} after {} ms: {error}",
            clear.elapsed_ms()
        )),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_new_session_for_the_same_agent_is_refused_until_the_first_ends() {
        let state = AppState::new();

        let first = ClearInFlight::begin(&state, "agent-1").expect("first claim");
        let error = ClearInFlight::begin(&state, "agent-1")
            .err()
            .expect("duplicate is refused");
        assert!(error.contains("already starting"), "{error}");
        ClearInFlight::begin(&state, "agent-2").expect("another agent is independent");

        drop(first);
        ClearInFlight::begin(&state, "agent-1").expect("claim is free after the first ends");
    }
}
