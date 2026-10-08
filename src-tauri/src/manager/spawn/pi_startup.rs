//! Own the startup lease until authenticated readiness or verified child exit.
use super::*;
use crate::delivery::pi_bridge::{PiBridgeOwner, PiBridgeStartup};
use crate::manager::codex_stop;
use std::sync::{Arc, Mutex};
use wardian_core::conversation_lease::PersistedConversationLeaseGuard;

pub(super) struct PiStartupContext<R: tauri::Runtime> {
    pub(super) status: Arc<Mutex<String>>,
    pub(super) session_id: String,
    pub(super) bootstrap: Arc<std::sync::atomic::AtomicBool>,
    pub(super) app: AppHandle<R>,
    pub(super) runtime_generation: u64,
    pub(super) gate: SpawnPublicationGate,
    pub(super) bridge: Arc<PiBridgeOwner>,
}

/// This task owns cleanup; callers receive only a cancellable completion observer.
pub(super) async fn monitor<R: tauri::Runtime>(
    mut lease: PersistedConversationLeaseGuard,
    context: PiStartupContext<R>,
) {
    let PiStartupContext {
        status,
        session_id,
        bootstrap,
        app,
        runtime_generation,
        gate,
        bridge,
    } = context;
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        lease.retain_until_expiry();
        return;
    };
    let owner = lease.owner().clone();
    let state = app.state::<AppState>();
    let mut startup = bridge.startup();
    let mut last_renewal = std::time::Instant::now();
    let mut stop = None;
    let mut terminal_status = None;
    let mut failure = None;
    loop {
        if let Some(handle) = stop.as_ref() {
            let handle: &codex_stop::StopHandle = handle;
            match handle.observe_exit() {
                Ok(true) => {
                    if let Some(current) = terminal_status.as_ref() {
                        publish_status(&app, &session_id, current, "Error", failure.as_deref())
                            .await;
                    }
                    let _ = lease.release();
                    return;
                }
                Ok(false) => {}
                Err(error) => log_debug(&format!("[Wardian] Pi exit observation: {error}")),
            }
        } else {
            let cancelled = gate
                .unpublished_failure
                .as_ref()
                .is_some_and(|value| value.load(std::sync::atomic::Ordering::Acquire))
                || gate
                    .registration
                    .as_ref()
                    .is_some_and(|value| value.state() == RegistrationPublicationState::FAILED);
            let committed = gate
                .registration
                .as_ref()
                .is_none_or(|value| value.state() == RegistrationPublicationState::COMMITTED);
            let bridge_state = startup.borrow_and_update().clone();
            let current = status.lock().map(|value| value.clone()).unwrap_or_default();
            let failed = match bridge_state {
                PiBridgeStartup::Failed(ref reason) => Some(reason.clone()),
                _ if cancelled => Some("Pi startup registration was cancelled".into()),
                _ if matches!(current.as_str(), "Off" | "Error") => {
                    Some("Pi exited or failed before startup completed".into())
                }
                _ => None,
            };
            if let Some(reason) = failed {
                // PendingRuntime cancellation captures synchronously. Its exact
                // receipt remains observable even when the stop already exited.
                match codex_stop::matching_pi_stop(&home, &session_id, runtime_generation, &status)
                {
                    Ok(Some(handle)) => stop = Some(handle),
                    Err(error) => log_debug(&format!("[Wardian] Pi retained stop lookup: {error}")),
                    Ok(None) => {}
                }
                if stop.is_none() && (committed || cancelled) {
                    let _lifecycle = state.lock_agent_lifecycle(&session_id).await;
                    if state
                        .interactions
                        .current_provider_input_generation(&session_id)
                        .await
                        == Some(bridge.generation())
                    {
                        let registration = codex_stop::prepare_pi_stop(&home, &session_id)
                            .and_then(|registration| {
                                registration.with_terminal_cleanup(state.terminal_sessions.clone())
                            });
                        if let Ok(registration) = registration {
                            let mut agents = state.agents.lock().await;
                            if let Some(agent) = agents.get_mut(&session_id).filter(|agent| {
                                agent.runtime_generation == Some(runtime_generation)
                                    && Arc::ptr_eq(&agent.current_status, &status)
                            }) {
                                let captured =
                                    crate::commands::agent::take_agent_runtime_for_termination(
                                        agent,
                                    );
                                // Change incarnation before any await: late reader
                                // EOF/status events cannot replace the failure.
                                let replacement = Arc::new(Mutex::new("Action Needed".into()));
                                agent.current_status = replacement.clone();
                                terminal_status = Some(replacement);
                                stop = Some(registration.capture(captured).begin_stop());
                            }
                        }
                    }
                }
                if let Some(handle) = stop.as_ref() {
                    failure = Some(reason);
                    let _ = state
                        .native_delivery
                        .dispose_pi_generation(&session_id, Some(bridge.generation()))
                        .await;
                    if let Some(current) = terminal_status.as_ref() {
                        publish_status(
                            &app,
                            &session_id,
                            current,
                            "Action Needed",
                            failure.as_deref(),
                        )
                        .await;
                    }
                    match handle.wait().await {
                        Ok(()) => {
                            let _ = handle.observe_exit();
                            if let Some(current) = terminal_status.as_ref() {
                                publish_status(
                                    &app,
                                    &session_id,
                                    current,
                                    "Error",
                                    failure.as_deref(),
                                )
                                .await;
                            }
                            let _ = lease.release();
                            return;
                        }
                        Err(error) => {
                            failure = Some(format!(
                                "{}; {error}. Child exit is unverified; ownership retained",
                                failure.as_deref().unwrap_or("Pi startup failed")
                            ));
                            if let Some(current) = terminal_status.as_ref() {
                                publish_status(
                                    &app,
                                    &session_id,
                                    current,
                                    "Action Needed",
                                    failure.as_deref(),
                                )
                                .await;
                            }
                        }
                    }
                }
            } else if committed
                && matches!(bridge_state, PiBridgeStartup::Ready)
                && (provider_spawn_lease_should_release(&current)
                    || bootstrap.load(std::sync::atomic::Ordering::Acquire))
            {
                let agents = state.agents.lock().await;
                if agents.get(&session_id).is_some_and(|agent| {
                    agent.runtime_generation == Some(runtime_generation)
                        && Arc::ptr_eq(&agent.current_status, &status)
                }) {
                    let _ = lease.release();
                    return;
                }
            }
        }
        if last_renewal.elapsed() >= PROVIDER_SPAWN_LEASE_HEARTBEAT {
            let now = chrono::Utc::now();
            match wardian_core::conversation_lease::renew_lease_owner_persisted(
                &owner,
                &now.to_rfc3339(),
                &(now + PROVIDER_SPAWN_LEASE_DURATION).to_rfc3339(),
            ) {
                Ok(true) => last_renewal = std::time::Instant::now(),
                result => {
                    log_debug(&format!("[Wardian] Pi startup lease renewal failed: {result:?}; retained stop fence remains"));
                    lease.retain_until_expiry();
                    return;
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {},
            _ = startup.changed(), if stop.is_none() => {},
        }
    }
}

/// Persist through the shared sequenced status path under lifecycle exclusion.
async fn publish_status<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    current: &Arc<Mutex<String>>,
    next: &str,
    failure: Option<&str>,
) {
    let state = app.state::<AppState>();
    let _lifecycle = state.lock_agent_lifecycle(session_id).await;
    {
        let agents = state.agents.lock().await;
        let Some(agent) = agents.get(session_id).filter(|agent| {
            agent.runtime_generation.is_none() && Arc::ptr_eq(&agent.current_status, current)
        }) else {
            return;
        };
        if let Ok(mut value) = current.lock() {
            state.reserve_status_intent(session_id, current, next);
            *value = next.into();
        }
        if let Some(failure) = failure {
            if let Ok(mut watch) = agent.watch_state.lock() {
                watch.push_event(
                    "startup_failure",
                    serde_json::json!({"message": failure, "exit_verified": next == "Error"}),
                );
            }
        }
    }
    let Some(revision) =
        crate::manager::commit_agent_status_publication(&state, session_id, current, next)
    else {
        return;
    };
    let sequence = state.next_status_observation_sequence(session_id);
    if crate::manager::persist_status_observation(
        &state,
        session_id,
        current,
        next,
        sequence,
        revision,
        &chrono::Utc::now().to_rfc3339(),
    )
    .await
    .is_some()
    {
        crate::manager::record_provider_input_from_status_state(&state, session_id, next, revision)
            .await;
        let _ = app.emit(
            "agent-status-updated",
            serde_json::json!({"session_id": session_id, "current_status": next}),
        );
    }
}
