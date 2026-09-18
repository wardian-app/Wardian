use super::agent_lifecycle::PendingRuntime;
use crate::manager;
use crate::state::{ActiveAgent, AppState};
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::models::AgentConfig;

#[cfg(test)]
#[path = "codex_onboarding_tests.rs"]
mod tests;

pub(super) async fn register_new_agent(
    mut config: AgentConfig,
    actual_resume: Option<String>,
    state: &AppState,
    app: &AppHandle,
    options: super::AgentRegistrationOptions<'_>,
) -> Result<AgentConfig, String> {
    let session_id = config.session_id.clone();
    config.system_include_directories = Some(crate::utils::fs::resolve_system_include_directories(
        &config.agent_class,
        &session_id,
    ));
    let pending = PendingRuntime::prepare(&config, &state.terminal_sessions)?;
    let (active_agent, mut completion) = spawn_for_registration(pending, app, &config).await?;
    // Propagate any fields that spawn_agent may have auto-assigned (e.g. opencode_port).

    {
        let mut cfg = active_agent.config.lock().unwrap();
        if config.provider == "opencode" {
            let opencode = cfg.opencode_config();
            config.opencode_port = opencode.port;
            if let wardian_core::models::ProviderConfig::OpenCode(target) =
                &mut config.provider_config
            {
                target.port = opencode.port;
            }
        }
        super::agent_lifecycle::sync_registered_provider_session(
            &mut config,
            &mut cfg,
            actual_resume,
        );
    }

    {
        let agents = state.agents.lock().await;
        if agents.contains_key(&session_id) {
            let error = format!("An agent with session ID '{session_id}' already exists.");
            return Err(stop_uncommitted_agent(completion, active_agent, error).await);
        }
    }
    let existing_names = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .map(|agent| agent.config.lock().unwrap().session_name.clone())
            .collect::<std::collections::HashSet<_>>()
    };
    match super::resolve_registered_session_name(
        &config.session_name,
        options.clone_name_base,
        &existing_names,
    ) {
        Ok(session_name) => config.session_name = session_name,
        Err(error) => {
            return Err(stop_uncommitted_agent(completion, active_agent, error).await);
        }
    }
    if let Some(reserved_session_name) = options.reserved_session_name {
        let mut reservations = state.agent_name_reservations.lock().await;
        reservations.remove(reserved_session_name);
    }
    {
        let mut cfg = active_agent.config.lock().unwrap();
        cfg.session_name = config.session_name.clone();
    }
    if let Err((pending, error)) = commit_registered_agent(
        state,
        &session_id,
        active_agent,
        &mut completion,
        options.placement,
    )
    .await
    {
        return Err(stop_uncommitted_agent(
            completion.take(),
            pending,
            format!("Failed to publish provisional agent: {error}"),
        )
        .await);
    }
    state.interactions.clear_deleted_session(&session_id).await;
    if options.emit_roster_update {
        let _ = app.emit("agents-updated", ());
    }

    await_completion(
        start_completion(completion),
        state,
        &session_id,
        &mut config,
    )
    .await?;

    Ok(config)
}

pub(super) async fn spawn_for_registration(
    pending: PendingRuntime,
    app: &AppHandle,
    config: &AgentConfig,
) -> Result<(PendingRuntime, Option<manager::CodexAttachmentCompletion>), String> {
    if manager::should_publish_provisionally(&config.provider, false) {
        let spawned =
            manager::spawn_agent_provisionally(app.clone(), config.clone(), false, None).await?;
        Ok((pending.attach(spawned.active), spawned.completion))
    } else {
        let active = manager::spawn_agent(app.clone(), config.clone(), false, None).await?;
        Ok((pending.attach(active), None))
    }
}

#[allow(clippy::result_large_err)]
pub(super) async fn commit_registered_agent(
    state: &AppState,
    session_id: &str,
    pending: PendingRuntime,
    completion: &mut Option<manager::CodexAttachmentCompletion>,
    placement: super::AgentOrderPlacement<'_>,
) -> Result<(), (PendingRuntime, String)> {
    let codex_runtime_generation = pending
        .config
        .lock()
        .ok()
        .filter(|config| config.provider == "codex")
        .map(|_| pending.runtime_generation);
    let lifecycle = state.lock_agent_lifecycle(session_id).await;
    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    let pending = install_registered_agent(&mut agents, &mut order, session_id, pending, placement);
    drop(order);
    drop(agents);

    let persisted = if let Some(runtime_generation) = codex_runtime_generation {
        drop(lifecycle);
        persist_provisional_codex_roster(state, session_id, runtime_generation).await
    } else {
        persist_current_provisional_roster(state).await
    };
    let mut pending = match persisted {
        Ok(()) => pending,
        Err(error) => {
            let pending = if let Some(runtime_generation) = codex_runtime_generation {
                rollback_failed_provisional_codex_install(
                    state,
                    session_id,
                    runtime_generation,
                    pending,
                )
                .await
            } else {
                rollback_failed_provisional_install(state, session_id, pending).await
            };
            return match pending {
                Ok(pending) => Err((pending, error)),
                Err((pending, repair_error)) => Err((
                    pending,
                    format!("{error}; failed to repair provisional roster: {repair_error}"),
                )),
            };
        }
    };
    if let Some(runtime_generation) = codex_runtime_generation {
        let lifecycle = state.lock_agent_lifecycle(session_id).await;
        let current = state
            .agents
            .lock()
            .await
            .get(session_id)
            .is_some_and(|agent| agent.runtime_generation == runtime_generation);
        if !current {
            return Err((
                pending,
                "provisional Codex runtime became stale before publication".into(),
            ));
        }
        if let Some(completion) = completion.as_mut() {
            completion.set_stop_registration(pending.take_registration());
        }
        drop(lifecycle);
    } else if let Some(completion) = completion.as_mut() {
        completion.set_stop_registration(pending.take_registration());
    }
    drop(pending);
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn rollback_failed_provisional_install(
    state: &AppState,
    session_id: &str,
    pending: PendingRuntime,
) -> Result<PendingRuntime, (PendingRuntime, String)> {
    let active = {
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        let active = agents
            .remove(session_id)
            .expect("provisional install remains current during commit rollback");
        order.retain(|id| id != session_id);
        active
    };
    let pending = pending.attach(active);
    match persist_current_provisional_roster(state).await {
        Ok(()) => Ok(pending),
        Err(error) => Err((pending, error)),
    }
}

#[allow(clippy::result_large_err)]
async fn rollback_failed_provisional_codex_install(
    state: &AppState,
    session_id: &str,
    runtime_generation: Option<u64>,
    pending: PendingRuntime,
) -> Result<PendingRuntime, (PendingRuntime, String)> {
    let active = {
        let _lifecycle = state.lock_agent_lifecycle(session_id).await;
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        let Some(current) = agents.get(session_id) else {
            return Err((
                pending,
                "provisional Codex runtime disappeared before rollback".into(),
            ));
        };
        if current.runtime_generation != runtime_generation {
            return Err((
                pending,
                "provisional Codex runtime became stale before rollback".into(),
            ));
        }
        let active = agents
            .remove(session_id)
            .expect("checked provisional Codex runtime");
        order.retain(|id| id != session_id);
        active
    };
    let pending = pending.attach(active);
    match persist_current_provisional_roster(state).await {
        Ok(()) => Ok(pending),
        Err(error) => Err((pending, error)),
    }
}

pub(super) fn install_registered_agent(
    agents: &mut HashMap<String, ActiveAgent>,
    order: &mut Vec<String>,
    session_id: &str,
    mut pending: PendingRuntime,
    placement: super::AgentOrderPlacement<'_>,
) -> PendingRuntime {
    agents.insert(session_id.to_string(), pending.take_runtime());
    super::insert_new_agent_order(order, session_id, placement);
    pending
}

pub(super) fn start_completion(
    completion: Option<manager::CodexAttachmentCompletion>,
) -> Option<tauri::async_runtime::JoinHandle<Result<(), String>>> {
    completion.map(|completion| tauri::async_runtime::spawn(completion.run()))
}

pub(super) async fn await_completion(
    completion: Option<tauri::async_runtime::JoinHandle<Result<(), String>>>,
    state: &AppState,
    session_id: &str,
    config: &mut AgentConfig,
) -> Result<(), String> {
    let Some(completion) = completion else {
        return Ok(());
    };
    completion
        .await
        .map_err(|error| format!("Codex attachment task failed: {error}"))??;
    *config = {
        let agents = state.agents.lock().await;
        let cloned = agents
            .get(session_id)
            .ok_or_else(|| "Codex agent disappeared after attachment".to_string())?
            .config
            .lock()
            .map_err(|_| "Agent configuration lock poisoned".to_string())?
            .clone();
        cloned
    };
    Ok(())
}

pub(super) async fn stop_uncommitted_agent(
    completion: Option<manager::CodexAttachmentCompletion>,
    pending: PendingRuntime,
    error: String,
) -> String {
    let error = if let Some(completion) = completion {
        match completion.cancel().await {
            Ok(()) => error,
            Err(cleanup) => format!("{error}; Codex attachment cleanup retained: {cleanup}"),
        }
    } else {
        error
    };
    pending.stop_after_failure(error).await
}

pub(crate) async fn rollback_provisional_codex(
    app: &AppHandle,
    session_id: &str,
    provider_generation: u64,
    runtime_generation: u64,
    registration: Option<crate::manager::codex_stop::StopRegistration>,
    failure: &str,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some(registration) = registration else {
        return Err("provisional Codex stop registration was not retained".to_string());
    };

    let detached = match detach_provisional_codex(
        &state,
        session_id,
        runtime_generation,
        registration,
    )
    .await
    {
        Ok(Some(detached)) => detached,
        Ok(None) => return Ok(()),
        Err(DetachProvisionalError::Persistence { detached, error }) => {
            return retain_failed_detached_codex(
                app,
                &state,
                session_id,
                provider_generation,
                format!("{failure}; failed to persist provisional rollback: {error}"),
                detached,
            )
            .await;
        }
        Err(DetachProvisionalError::Infrastructure {
            registration,
            error,
        }) => {
            return retain_failed_provisional_codex(
                app,
                &state,
                session_id,
                provider_generation,
                runtime_generation,
                registration,
                format!("{failure}; failed to prepare provisional rollback: {error}"),
            )
            .await;
        }
    };

    if let Err(error) = state
        .interactions
        .delete_agent_durable_state(session_id)
        .await
    {
        return retain_failed_detached_codex(
            app,
            &state,
            session_id,
            provider_generation,
            format!("{failure}; failed to delete provisional interaction state: {error}"),
            detached,
        )
        .await;
    }

    state.remove_agent_delivery_state(session_id).await;
    let terminal_error = if let Some(generation) = detached.generation {
        state
            .terminal_sessions
            .terminate_and_remove_runtime(session_id, generation)
            .await
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    if let Some(error) = terminal_error {
        return retain_failed_detached_codex(
            app,
            &state,
            session_id,
            provider_generation,
            format!("{failure}; terminal cleanup failed: {error}"),
            detached,
        )
        .await;
    }
    let stop_error = detached.stop_guard.begin_stop().wait().await.err();
    let _ = app.emit("agents-updated", ());
    match stop_error {
        None => Ok(()),
        Some(stop) => Err(manager::codex_onboarding::format_cleanup_errors(
            failure,
            None,
            None,
            Some(stop),
        )),
    }
}

struct DetachedProvisionalCodex {
    shell: Option<ActiveAgent>,
    stop_guard: crate::manager::codex_stop::StopGuard,
    generation: Option<u64>,
    order_index: usize,
}

enum DetachProvisionalError {
    Persistence {
        detached: DetachedProvisionalCodex,
        error: String,
    },
    Infrastructure {
        registration: crate::manager::codex_stop::StopRegistration,
        error: String,
    },
}

#[allow(clippy::result_large_err)]
async fn detach_provisional_codex(
    state: &AppState,
    session_id: &str,
    runtime_generation: u64,
    registration: crate::manager::codex_stop::StopRegistration,
) -> Result<Option<DetachedProvisionalCodex>, DetachProvisionalError> {
    let mut registration = Some(registration);
    loop {
        let lifecycle = state.lock_agent_lifecycle(session_id).await;
        let (barrier_busy, result) = {
            let agents = state.agents.lock().await;
            let order = state.agent_order.lock().await;
            let barrier = match wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
            {
                Ok(barrier) => barrier,
                Err(error) => {
                    return Err(DetachProvisionalError::Infrastructure {
                        registration: registration
                            .take()
                            .expect("rollback registration retained before detach"),
                        error: error.to_string(),
                    });
                }
            };
            match barrier {
                Some(_barrier) => {
                    let current = agents.get(session_id).is_some_and(|agent| {
                        manager::codex_onboarding::exact_runtime_generation_matches(
                            agent.runtime_generation,
                            runtime_generation,
                        )
                    });
                    if !current {
                        (false, Ok(None))
                    } else {
                        let mut agents = agents;
                        let mut order = order;
                        let mut agent = agents
                            .remove(session_id)
                            .expect("checked provisional agent");
                        let order_index = order
                            .iter()
                            .position(|id| id == session_id)
                            .unwrap_or(order.len());
                        order.retain(|id| id != session_id);
                        let generation = agent.runtime_generation;
                        let stop_guard = registration
                            .take()
                            .expect("rollback registration retained for current runtime")
                            .capture(super::take_agent_runtime_for_termination(&mut agent));
                        let detached = DetachedProvisionalCodex {
                            shell: Some(agent),
                            stop_guard,
                            generation,
                            order_index,
                        };
                        let snapshot = manager::state_configs_snapshot(&agents, &order);
                        if let Err(error) = manager::try_save_state_snapshot_unlocked(&snapshot) {
                            return Err(DetachProvisionalError::Persistence { detached, error });
                        }
                        (false, Ok(Some(detached)))
                    }
                }
                None => (true, Ok(None)),
            }
        };
        drop(lifecycle);
        if !barrier_busy {
            return result;
        }
        if let Err(error) = tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(true).map(drop)
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))
        {
            return Err(DetachProvisionalError::Infrastructure {
                registration: registration
                    .take()
                    .expect("rollback registration retained while waiting for roster barrier"),
                error,
            });
        }
    }
}

async fn persist_current_provisional_roster(state: &AppState) -> Result<(), String> {
    loop {
        let (barrier_busy, result) = {
            let agents = state.agents.lock().await;
            let order = state.agent_order.lock().await;
            match wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                .map_err(|error| error.to_string())?
            {
                Some(_barrier) => {
                    let snapshot = manager::state_configs_snapshot(&agents, &order);
                    (false, manager::try_save_state_snapshot_unlocked(&snapshot))
                }
                None => (true, Ok(())),
            }
        };
        if !barrier_busy {
            return result;
        }
        tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(true).map(drop)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
}

async fn persist_provisional_codex_roster(
    state: &AppState,
    session_id: &str,
    runtime_generation: Option<u64>,
) -> Result<(), String> {
    loop {
        let lifecycle = state.lock_agent_lifecycle(session_id).await;
        let (barrier_busy, result) = {
            match wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                .map_err(|error| error.to_string())?
            {
                Some(_barrier) => {
                    let agents = state.agents.lock().await;
                    let order = state.agent_order.lock().await;
                    let current = agents
                        .get(session_id)
                        .is_some_and(|agent| agent.runtime_generation == runtime_generation);
                    if !current {
                        (
                            false,
                            Err("provisional Codex runtime became stale before persistence".into()),
                        )
                    } else {
                        let snapshot = manager::state_configs_snapshot(&agents, &order);
                        (false, manager::try_save_state_snapshot_unlocked(&snapshot))
                    }
                }
                None => (true, Ok(())),
            }
        };
        drop(lifecycle);
        if !barrier_busy {
            return result;
        }
        tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(true).map(drop)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
}

async fn retain_failed_detached_codex(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    provider_generation: u64,
    failure: String,
    mut detached: DetachedProvisionalCodex,
) -> Result<(), String> {
    let status = detached
        .shell
        .as_ref()
        .map(|shell| shell.current_status.clone())
        .ok_or_else(|| "failed Codex roster shell was already retained".to_string())?;
    manager::set_agent_status(app, session_id, &status, "Error");
    let roster_error = retain_failed_detached_roster(state, session_id, &mut detached)
        .await
        .err();
    state
        .interactions
        .record_provider_input_state(
            session_id,
            provider_generation,
            wardian_core::control::ProviderInputReadiness::Unavailable,
            None,
        )
        .await;
    let terminal_error = if let Some(generation) = detached.generation {
        state
            .terminal_sessions
            .terminate_and_remove_runtime(session_id, generation)
            .await
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    let stop_error = detached.stop_guard.begin_stop().wait().await.err();
    let _ = app.emit("agents-updated", ());
    match (roster_error, terminal_error, stop_error) {
        (None, None, None) => Err(failure),
        (roster, terminal, stop) => Err(manager::codex_onboarding::format_cleanup_errors(
            &failure, roster, terminal, stop,
        )),
    }
}

async fn retain_failed_detached_roster(
    state: &AppState,
    session_id: &str,
    detached: &mut DetachedProvisionalCodex,
) -> Result<(), String> {
    loop {
        let _lifecycle = state.lock_agent_lifecycle(session_id).await;
        let (barrier_busy, result) = {
            match wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                .map_err(|error| error.to_string())?
            {
                Some(_barrier) => {
                    let mut agents = state.agents.lock().await;
                    let mut order = state.agent_order.lock().await;
                    if !agents.contains_key(session_id) {
                        let shell = detached
                            .shell
                            .take()
                            .expect("checked detached provisional shell");
                        agents.insert(session_id.to_string(), shell);
                        let index = detached.order_index.min(order.len());
                        order.insert(index, session_id.to_string());
                    }
                    let snapshot = manager::state_configs_snapshot(&agents, &order);
                    (false, manager::try_save_state_snapshot_unlocked(&snapshot))
                }
                None => (true, Ok(())),
            }
        };
        drop(_lifecycle);
        if !barrier_busy {
            return result;
        }
        tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(true).map(drop)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
}

async fn retain_failed_provisional_codex(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    provider_generation: u64,
    runtime_generation: u64,
    registration: crate::manager::codex_stop::StopRegistration,
    failure: String,
) -> Result<(), String> {
    let (stop_guard, generation, status) = {
        let _lifecycle = state.lock_agent_lifecycle(session_id).await;
        let mut agents = state.agents.lock().await;
        let Some(agent) = agents.get_mut(session_id) else {
            return Ok(());
        };
        if !manager::codex_onboarding::exact_runtime_generation_matches(
            agent.runtime_generation,
            runtime_generation,
        ) {
            return Ok(());
        }
        let status = agent.current_status.clone();
        manager::set_agent_status(app, session_id, &status, "Error");
        let generation = agent.runtime_generation;
        let detached = super::take_agent_runtime_for_termination(agent);
        let stop_guard = registration.capture(detached);
        (stop_guard, generation, status)
    };

    let roster_error = persist_current_provisional_roster(state).await.err();
    state
        .interactions
        .record_provider_input_state(
            session_id,
            provider_generation,
            wardian_core::control::ProviderInputReadiness::Unavailable,
            None,
        )
        .await;
    let terminal_error = if let Some(generation) = generation {
        state
            .terminal_sessions
            .terminate_and_remove_runtime(session_id, generation)
            .await
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    let stop_error = stop_guard.begin_stop().wait().await.err();
    manager::set_agent_status(app, session_id, &status, "Error");
    let _ = app.emit("agents-updated", ());
    if roster_error.is_none() && terminal_error.is_none() && stop_error.is_none() {
        Ok(())
    } else {
        Err(manager::codex_onboarding::format_cleanup_errors(
            &failure,
            roster_error,
            terminal_error,
            stop_error,
        ))
    }
}
