//! Restore a dismissed Codex menu from its exact shared owner, never screen text alone.
use super::startup_readiness::ProviderStartupObservation;
use crate::delivery::codex_shared::{CodexTurnActivity, Observation};
use crate::state::AppState;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::watch;

/// Called only after the attached TUI repaints an explicit choice as a composer.
/// A lost/replaced owner or incomplete native state leaves the action pending.
pub(crate) async fn restore_after_choice(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
) -> bool {
    restore_with_owner(Some(app), state, session_id, observation, async {
        state
            .native_delivery
            .codex_observations(session_id, observation.input_generation)
            .await
            .map_err(|error| error.to_string())
    })
    .await
}

async fn runtime_matches(
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
) -> bool {
    if state
        .interactions
        .current_provider_input_generation(session_id)
        .await
        != Some(observation.input_generation)
    {
        return false;
    }
    let agents = state.agents.lock().await;
    agents.get(session_id).is_some_and(|agent| {
        agent.runtime_generation == Some(observation.runtime_generation)
            && Arc::ptr_eq(&agent.current_status, &observation.current_status)
            && agent
                .config
                .lock()
                .is_ok_and(|config| config.provider == "codex")
            && agent.current_status.lock().is_ok_and(|status| {
                wardian_core::identity::normalize_status(&status) == "action_required"
            })
    })
}

async fn restore_with_owner(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
    owner: impl std::future::Future<Output = Result<watch::Receiver<Observation>, String>>,
) -> bool {
    let _lifecycle = state.lock_agent_lifecycle(session_id).await;
    if !runtime_matches(state, session_id, observation).await {
        return false;
    }
    let Ok(owner) = owner.await else {
        return false;
    };
    // Owner lookup can await. Revalidate both identities and the current screen
    // before consulting the latest watch value and publishing without another await.
    if !runtime_matches(state, session_id, observation).await {
        return false;
    }
    let Ok(snapshot) = state.terminal_sessions.snapshot(session_id).await else {
        return false;
    };
    if snapshot.runtime_generation != observation.runtime_generation
        || crate::delivery::codex_menu::current_screen_requires_choice(&snapshot.visible_grid)
        || !crate::delivery::codex_composer::output_has_ready_prompt(&snapshot.visible_grid)
    {
        return false;
    }
    let status = match owner.borrow().activity() {
        CodexTurnActivity::Idle(_) => "Idle",
        CodexTurnActivity::Processing(_) => "Processing...",
        CodexTurnActivity::Pending | CodexTurnActivity::Closed | CodexTurnActivity::Stopped => {
            return false
        }
    };
    if let Some(app) = app {
        crate::manager::set_agent_status(app, session_id, &observation.current_status, status);
    } else {
        *observation.current_status.lock().unwrap() = status.into();
    }
    true
}

/// Apply the menu constraint to the same status Arc that scheduled publication.
/// The manager holds the lifecycle lock; replacement cannot redirect this update.
pub(crate) async fn constrain_publication(
    state: &AppState,
    session_id: &str,
    current_status: &Arc<std::sync::Mutex<String>>,
    requested: &str,
) -> Option<String> {
    {
        let agents = state.agents.lock().await;
        let agent = agents.get(session_id)?;
        if !Arc::ptr_eq(&agent.current_status, current_status) {
            return None;
        }
    }
    let required =
        super::startup_readiness::constrain_codex_status_observation(state, session_id, requested)
            .await;
    let mut current = current_status.lock().ok()?;
    if *current != requested {
        return None;
    }
    if let Some(required) = required {
        *current = required.into();
    }
    Some(current.clone())
}

#[cfg(test)]
#[path = "codex_menu_status_tests.rs"]
mod tests;
