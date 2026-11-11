use crate::state::AppState;
use tauri::{AppHandle, Manager};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

pub(crate) fn recover_after_owner_acquired() -> Result<(), String> {
    wardian_core::agent_spawn::recover_after_owner_acquired()?;
    // A sealed deletion is metadata-only and precedes ordinary name repair or
    // restore. Unsealed dispositions retain their exclusion without replay.
    if let Err(error) = crate::commands::agent::recover_bound_removals() {
        crate::manager::log_debug(&format!(
            "[WARDIAN] bound deletion recovery retained: {error}"
        ));
    }
    let recovered = persisted_roster().and_then(|roster| {
        wardian_core::agent_spawn::recover_legacy_names(&roster)?;
        wardian_core::agent_spawn::recover_first_native_bindings(&roster)
    });
    if let Err(error) = recovered {
        // Each repair transaction is atomic. Its pending owner link already
        // excludes restore, including when the stores cannot be read/repaired.
        crate::manager::log_debug(&format!("[WARDIAN] legacy name recovery retained: {error}"));
    }
    Ok(())
}

/// Read the actual separately published roster. An unreadable or malformed
/// roster cannot supply name-ownership recovery evidence.
pub(crate) fn persisted_roster() -> Result<Vec<wardian_core::models::AgentConfig>, String> {
    let home = crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
    match std::fs::read(home.join("settings/state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "Could not decode persisted agent roster".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err("Could not read persisted agent roster".to_string()),
    }
}

pub(crate) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let queued = tokio::task::spawn_blocking(wardian_core::agent_spawn::queued_operation_ids)
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        match queued {
            Ok(operation_ids) => {
                for operation_id in operation_ids {
                    schedule(app.clone(), operation_id);
                }
            }
            Err(error) => crate::manager::log_debug(&format!(
                "[WARDIAN] durable spawn queue recovery failed: {error}"
            )),
        }
    });
}

pub(crate) fn schedule(app: AppHandle, operation_id: String) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let Ok(_permit) = state.spawn_operation_slots.clone().acquire_owned().await else {
            return;
        };
        run_operation(app, operation_id).await;
    });
}

pub(crate) async fn wait_for_operation(
    operation_id: &str,
    timeout_ms: u64,
) -> Result<
    (
        wardian_core::agent_spawn::AgentSpawnOperation,
        bool,
        tokio::time::Instant,
    ),
    crate::control::ControlError,
> {
    if timeout_ms > wardian_core::agent_spawn::MAX_OBSERVER_WAIT_MS {
        return Err(crate::control::ControlError::bad_request(format!(
            "spawn observer wait is limited to {} ms",
            wardian_core::agent_spawn::MAX_OBSERVER_WAIT_MS
        )));
    }
    let operation_id = operation_id.to_string();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let id = operation_id.clone();
        let operation = observe_before_deadline(
            deadline,
            tokio::task::spawn_blocking(move || wardian_core::agent_spawn::get(&id)),
        )
        .await?
        .map_err(|error| {
            crate::control::ControlError::request_failed(format!(
                "spawn operation lookup failed: {error}"
            ))
        })?
        .map_err(crate::control::ControlError::request_failed)?
        .ok_or_else(|| crate::control::ControlError::not_found("spawn operation not found"))?;
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok((operation, true, deadline));
        }
        if operation.state.is_terminal() {
            return Ok((operation, false, deadline));
        }
        tokio::time::sleep(POLL_INTERVAL.min(deadline - now)).await;
        if tokio::time::Instant::now() >= deadline {
            return Ok((operation, true, deadline));
        }
    }
}

/// An observer budget never cancels or replays the admitted spawn worker.
pub(crate) async fn observe_before_deadline<T>(
    deadline: tokio::time::Instant,
    observation: impl std::future::Future<Output = T>,
) -> Result<T, crate::control::ControlError> {
    tokio::time::timeout_at(deadline, observation)
        .await
        .map_err(|_| {
            crate::control::ControlError::coded(
                "wait_timeout",
                "spawn observation deadline expired",
            )
        })
}

struct WorkerOutcomeGuard {
    operation_id: String,
    attempt_id: String,
    terminal: bool,
}

impl Drop for WorkerOutcomeGuard {
    fn drop(&mut self) {
        if !self.terminal {
            let _ = wardian_core::agent_spawn::mark_outcome_unknown(
                &self.operation_id,
                &self.attempt_id,
            );
        }
    }
}

async fn run_operation(app: AppHandle, operation_id: String) {
    let operation_key = operation_id.clone();
    let claimed = tokio::task::spawn_blocking(move || {
        wardian_core::agent_spawn::claim_queued(&operation_key)
    })
    .await;
    let plan = match claimed {
        Ok(Ok(Some(plan))) => plan,
        Ok(Ok(None)) => return,
        Ok(Err(error)) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] spawn operation claim failed for {operation_id}: {error}"
            ));
            return;
        }
        Err(error) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] spawn operation claim worker failed for {operation_id}: {error}"
            ));
            return;
        }
    };
    let Some(attempt_id) = plan.attempt_id.clone() else {
        crate::manager::log_debug(&format!(
            "[WARDIAN] claimed spawn operation {operation_id} has no attempt id"
        ));
        let _ = tokio::task::spawn_blocking(move || {
            wardian_core::agent_spawn::mark_orphaned_claim_unknown(&operation_id)
        })
        .await;
        return;
    };
    let mut outcome_guard = WorkerOutcomeGuard {
        operation_id: operation_id.clone(),
        attempt_id: attempt_id.clone(),
        terminal: false,
    };

    let preparation_plan = plan.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        crate::commands::agent::prepare_durable_agent_spawn(
            preparation_plan.request.clone(),
            preparation_plan,
        )
    })
    .await;
    let prepared = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(error)) => {
            let (code, message) = safe_preflight_error(&error);
            let operation_key = operation_id.clone();
            let attempt_key = attempt_id.clone();
            let result = tokio::task::spawn_blocking(move || {
                wardian_core::agent_spawn::fail_before_launch(
                    &operation_key,
                    &attempt_key,
                    code,
                    message,
                )
            })
            .await;
            if matches!(result, Ok(Ok(true))) {
                outcome_guard.terminal = true;
            }
            return;
        }
        Err(error) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] spawn operation preparation worker failed for {operation_id}: {error}"
            ));
            return;
        }
    };

    let operation_key = operation_id.clone();
    let attempt_key = attempt_id.clone();
    match tokio::task::spawn_blocking(move || {
        wardian_core::agent_spawn::mark_launching(&operation_key, &attempt_key)
    })
    .await
    {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return,
        Ok(Err(error)) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] spawn operation launch fence failed for {operation_id}: {error}"
            ));
            return;
        }
        Err(error) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] spawn operation launch fence worker failed for {operation_id}: {error}"
            ));
            return;
        }
    }

    let state = app.state::<AppState>();
    let durable_operation = Some((operation_id.clone(), attempt_id.clone()));
    let agent = crate::commands::agent::launch_prepared_agent_spawn(
        prepared,
        state.inner(),
        &app,
        None,
        durable_operation,
    )
    .await;
    let registration = match agent {
        Ok(agent) => agent,
        Err(error) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] spawn operation {operation_id} outcome is unconfirmed: {}",
                safe_launch_error(&error)
            ));
            let operation_key = operation_id.clone();
            let attempt_key = attempt_id.clone();
            if matches!(
                tokio::task::spawn_blocking(move || {
                    wardian_core::agent_spawn::mark_outcome_unknown(&operation_key, &attempt_key)
                })
                .await,
                Ok(Ok(true))
            ) {
                outcome_guard.terminal = true;
            }
            return;
        }
    };
    let agent = &registration.config;

    let state = app.state::<AppState>();
    let _lifecycle = state.lock_agent_lifecycle(&agent.session_id).await;
    let _roster = match crate::manager::roster_io::acquire_roster_barrier().await {
        Ok(barrier) => barrier,
        Err(_) => return,
    };
    let agent = {
        let agents = state.agents.lock().await;
        let Some(active) = agents.get(&agent.session_id) else {
            return;
        };
        if !registration.matches(active) {
            return;
        }
        let config = active
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        drop(agents);
        config
    };
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        return;
    };
    let state_path = home.join("settings").join("state.json");
    let expected_session_id = agent.session_id.clone();
    let expected_session_name = agent.session_name.clone();
    let expected_provider = agent.provider.clone();
    let expected_class = agent.agent_class.clone();
    let expected_folder = agent.folder.clone();
    let expected_resume = agent.resume_session.clone();
    let expected_fresh = agent.fresh_provider_session_id.clone();
    let saved = tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(state_path)
            .ok()
            .and_then(|text| {
                serde_json::from_str::<Vec<wardian_core::models::AgentConfig>>(&text).ok()
            })
            .is_some_and(|configs| {
                configs.iter().any(|config| {
                    config.session_id == expected_session_id
                        && config.session_name == expected_session_name
                        && config.provider == expected_provider
                        && config.agent_class == expected_class
                        && config.folder == expected_folder
                        && config.resume_session == expected_resume
                        && config.fresh_provider_session_id == expected_fresh
                })
            })
    })
    .await
    .unwrap_or(false);
    if !saved {
        return;
    }
    let current = {
        let agents = state.agents.lock().await;
        agents
            .get(&agent.session_id)
            .is_some_and(|active| registration.matches(active))
    };
    if !current {
        return;
    }
    let operation_key = operation_id.clone();
    let attempt_key = attempt_id.clone();
    let provider_session_id = agent
        .resume_session
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(agent.fresh_provider_session_id.as_deref())
        .map(str::to_string);
    match tokio::task::spawn_blocking(move || {
        let _lifecycle = _lifecycle;
        let _roster = _roster;
        wardian_core::agent_spawn::mark_registered(
            &operation_key,
            &attempt_key,
            provider_session_id.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(true)) => outcome_guard.terminal = true,
        Ok(Ok(false)) => crate::manager::log_debug(&format!(
            "[WARDIAN] spawn operation {operation_id} registration did not match its durable plan"
        )),
        Ok(Err(error)) => crate::manager::log_debug(&format!(
            "[WARDIAN] spawn operation {operation_id} registration commit failed: {error}"
        )),
        Err(error) => crate::manager::log_debug(&format!(
            "[WARDIAN] spawn operation {operation_id} registration worker failed: {error}"
        )),
    }
}

fn safe_preflight_error(error: &str) -> (&'static str, &'static str) {
    let lower = error.to_ascii_lowercase();
    if lower.contains("class") {
        (
            "invalid_class",
            "Agent class validation failed before launch.",
        )
    } else if lower.contains("workspace") || lower.contains("folder") {
        (
            "invalid_workspace",
            "Workspace validation failed before launch.",
        )
    } else if lower.contains("provider") {
        (
            "provider_unavailable",
            "Provider validation failed before launch.",
        )
    } else {
        (
            "spawn_validation_failed",
            "Spawn request validation failed before launch.",
        )
    }
}

fn safe_launch_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("bootstrap") || lower.contains("session") {
        "provider bootstrap did not produce a verified registration"
    } else if lower.contains("roster") || lower.contains("persist") {
        "normal roster publication did not complete"
    } else {
        "provider launch or registration did not complete"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::agent_spawn::AgentSpawnState;

    #[test]
    fn preflight_diagnostics_never_persist_paths_or_provider_output() {
        assert_eq!(
            safe_preflight_error("workspace not found at C:/private/project"),
            (
                "invalid_workspace",
                "Workspace validation failed before launch."
            )
        );
        assert_eq!(
            safe_launch_error("provider emitted token at C:/private/provider.log"),
            "provider launch or registration did not complete"
        );
    }

    #[test]
    fn interrupted_worker_outcome_is_not_reported_as_registered() {
        let state = AgentSpawnState::OutcomeUnknown;
        assert!(state.is_terminal());
        assert_ne!(state, AgentSpawnState::Registered);
    }
}
