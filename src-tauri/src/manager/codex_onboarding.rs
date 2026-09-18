use crate::state::{ActiveAgent, AgentWatchState, AppState};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Manager, Runtime};
use wardian_core::control::ProviderInputReadiness;
use wardian_core::models::AgentConfig;

use super::codex_shared::CodexAttachGuard;

#[cfg(test)]
#[path = "codex_onboarding_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnPublication {
    Synchronous,
    Provisional,
}

pub(crate) struct SpawnedAgent {
    pub(crate) active: ActiveAgent,
    pub(crate) completion: Option<CodexAttachmentCompletion>,
}

pub(super) struct CodexAttachmentCompletionContext {
    pub(super) app: AppHandle,
    pub(super) session_id: String,
    pub(super) provider_generation: u64,
    pub(super) runtime_generation: u64,
    pub(super) config_lock: Arc<std::sync::Mutex<AgentConfig>>,
    pub(super) current_status: Arc<std::sync::Mutex<String>>,
    pub(super) watch_state: Arc<std::sync::Mutex<AgentWatchState>>,
    pub(super) native_delivery: Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    pub(super) codex_attachment_ready: Arc<AtomicBool>,
    pub(super) codex_reader_alive: Arc<AtomicBool>,
    pub(super) cleanup_guard: Option<CodexAttachGuard>,
}

/// The readiness gate belongs to the runtime's existing watch-state Arc so
/// status, telemetry, and native admission all observe the same incarnation.
pub(crate) fn codex_attachment_is_ready(agent: &ActiveAgent) -> bool {
    let Ok(config) = agent.config.lock() else {
        return false;
    };
    if config.provider != "codex" {
        return true;
    }
    drop(config);
    agent
        .watch_state
        .lock()
        .is_ok_and(|watch_state| watch_state.codex_attachment_ready())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodexStatusAdmission {
    Allowed,
    Blocked,
    WaitForRoster,
}

pub(crate) fn status_admission<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    current_status: &Arc<std::sync::Mutex<String>>,
    next_status: &str,
) -> CodexStatusAdmission {
    if !matches!(
        wardian_core::identity::normalize_status(next_status).as_str(),
        "idle" | "processing"
    ) {
        return CodexStatusAdmission::Allowed;
    }
    let state = app.state::<AppState>();
    let Ok(agents) = state.agents.try_lock() else {
        return CodexStatusAdmission::WaitForRoster;
    };
    let Some(agent) = agents.get(session_id) else {
        // Synchronous startup publishes before its ActiveAgent enters the
        // roster. There is no provisional runtime to fence in this window.
        return CodexStatusAdmission::Allowed;
    };
    if !Arc::ptr_eq(&agent.current_status, current_status) {
        return CodexStatusAdmission::Allowed;
    }
    if !codex_attachment_is_ready(agent) {
        CodexStatusAdmission::Blocked
    } else {
        CodexStatusAdmission::Allowed
    }
}

pub(crate) fn defer_status_transition(
    app: &AppHandle,
    session_id: &str,
    current_status: &Arc<std::sync::Mutex<String>>,
    next_status: String,
) {
    let app = app.clone();
    let session_id = session_id.to_string();
    let current_status = current_status.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let allowed = {
            let agents = state.agents.lock().await;
            agents.get(&session_id).is_some_and(|agent| {
                Arc::ptr_eq(&agent.current_status, &current_status)
                    && codex_attachment_is_ready(agent)
            })
        };
        if !allowed {
            return;
        }
        if let Ok(mut status) = current_status.lock() {
            if *status == next_status {
                return;
            }
            *status = next_status.clone();
        } else {
            return;
        }
        super::schedule_agent_status_observation(&app, &session_id, &current_status, next_status);
    });
}

pub(crate) fn defer_status_publication(
    app: &AppHandle,
    session_id: &str,
    current_status: &Arc<std::sync::Mutex<String>>,
) {
    let app = app.clone();
    let session_id = session_id.to_string();
    let current_status = current_status.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let allowed = {
            let agents = state.agents.lock().await;
            agents.get(&session_id).is_some_and(|agent| {
                Arc::ptr_eq(&agent.current_status, &current_status)
                    && codex_attachment_is_ready(agent)
            })
        };
        if !allowed {
            return;
        }
        let Ok(status) = current_status.lock().map(|status| status.clone()) else {
            return;
        };
        super::schedule_agent_status_observation(&app, &session_id, &current_status, status);
    });
}

pub(crate) struct CodexAttachmentCompletion {
    app: AppHandle,
    session_id: String,
    provider_generation: u64,
    runtime_generation: u64,
    config_lock: Arc<std::sync::Mutex<AgentConfig>>,
    current_status: Arc<std::sync::Mutex<String>>,
    watch_state: Arc<std::sync::Mutex<AgentWatchState>>,
    native_delivery: Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    codex_attachment_ready: Arc<AtomicBool>,
    codex_reader_alive: Arc<AtomicBool>,
    cleanup_guard: Option<CodexAttachGuard>,
    stop_registration: Option<crate::manager::codex_stop::StopRegistration>,
}

impl SpawnedAgent {
    pub(crate) fn without_completion(active: ActiveAgent) -> Self {
        Self {
            active,
            completion: None,
        }
    }
}

impl CodexAttachmentCompletion {
    pub(super) fn new(context: CodexAttachmentCompletionContext) -> Self {
        Self {
            app: context.app,
            session_id: context.session_id,
            provider_generation: context.provider_generation,
            runtime_generation: context.runtime_generation,
            config_lock: context.config_lock,
            current_status: context.current_status,
            watch_state: context.watch_state,
            native_delivery: context.native_delivery,
            codex_attachment_ready: context.codex_attachment_ready,
            codex_reader_alive: context.codex_reader_alive,
            cleanup_guard: context.cleanup_guard,
            stop_registration: None,
        }
    }

    pub(crate) fn set_stop_registration(
        &mut self,
        registration: Option<crate::manager::codex_stop::StopRegistration>,
    ) {
        self.stop_registration = registration;
    }

    pub(crate) async fn run(mut self) -> Result<(), String> {
        let state_app = self.app.clone();
        let state = state_app.state::<AppState>();
        let liveness_app = self.app.clone();
        let liveness_session = self.session_id.clone();
        let liveness_runtime_generation = self.runtime_generation;
        let liveness_reader_alive = self.codex_reader_alive.clone();
        let finalized = self
            .native_delivery
            .finalize_codex_tui(
                &self.session_id,
                self.provider_generation,
                move || {
                    let app = liveness_app.clone();
                    let session_id = liveness_session.clone();
                    let reader_alive = liveness_reader_alive.clone();
                    async move {
                        let state = app.state::<AppState>();
                        if !reader_alive.load(Ordering::Acquire) {
                            return Err(
                                crate::delivery::codex_shared::CodexSharedError::unsupported(
                                    "captured Codex PTY reader exited during attachment",
                                ),
                            );
                        }
                        let mut agents = state.agents.lock().await;
                        let agent = agents.get_mut(&session_id).ok_or_else(|| {
                            crate::delivery::codex_shared::CodexSharedError::unsupported(
                                "captured Codex runtime is no longer registered",
                            )
                        })?;
                        if !exact_runtime_generation_matches(
                            agent.runtime_generation,
                            liveness_runtime_generation,
                        ) {
                            return Err(
                                crate::delivery::codex_shared::CodexSharedError::unsupported(
                                    "captured Codex runtime generation is stale",
                                ),
                            );
                        }
                        let child = agent.child_process.as_mut().ok_or_else(|| {
                            crate::delivery::codex_shared::CodexSharedError::unsupported(
                                "captured Codex child is no longer registered",
                            )
                        })?;
                        match child.try_wait() {
                            Ok(None) => Ok(()),
                            _ => Err(
                                crate::delivery::codex_shared::CodexSharedError::unsupported(
                                    "captured ordinary Codex TUI exited during attachment",
                                ),
                            ),
                        }
                    }
                },
                |id| {
                    let mut captured_config = self.config_lock.lock().map_err(|_| {
                        crate::delivery::codex_shared::CodexSharedError::unsupported(
                            "agent config lock unavailable",
                        )
                    })?;
                    super::apply_provider_identity("codex", &mut captured_config, id)
                        .map(|_| ())
                        .map_err(crate::delivery::codex_shared::CodexSharedError::unsupported)
                },
            )
            .await;
        if let Err(error) = finalized {
            let failure = format_codex_attachment_error(error.to_string(), &self.watch_state);
            return self.fail_with_rollback(failure).await;
        }

        if !self.codex_reader_alive.load(Ordering::Acquire) {
            let failure = format_codex_attachment_error(
                "captured Codex PTY reader exited during finalization",
                &self.watch_state,
            );
            return self.fail_with_rollback(failure).await;
        }

        let observations = match self
            .native_delivery
            .codex_observations(&self.session_id, self.provider_generation)
            .await
        {
            Ok(observations) => observations,
            Err(error) => {
                let failure = format_codex_attachment_error(error.to_string(), &self.watch_state);
                return self.fail_with_rollback(failure).await;
            }
        };

        if let Err(error) = self.promote_after_attachment(&state).await {
            let failure = format_codex_attachment_error(error, &self.watch_state);
            return self.fail_with_rollback(failure).await;
        }
        super::codex_shared::observe_turn_activity(
            self.app.clone(),
            self.session_id.clone(),
            self.current_status.clone(),
            observations,
        );
        Ok(())
    }

    async fn promote_after_attachment(&mut self, state: &AppState) -> Result<(), String> {
        let _lifecycle = state.lock_agent_lifecycle(&self.session_id).await;
        if !self.codex_reader_alive.load(Ordering::Acquire) {
            return Err("captured Codex PTY reader exited during finalization".into());
        }
        persist_codex_identity(
            state,
            &self.session_id,
            self.runtime_generation,
            &self.config_lock,
        )
        .await?;
        assert_live_codex_incarnation(
            state,
            &self.session_id,
            self.runtime_generation,
            &self.config_lock,
        )
        .await?;

        let readiness = state
            .interactions
            .record_provider_input_state(
                &self.session_id,
                self.provider_generation,
                ProviderInputReadiness::Ready,
                None,
            )
            .await;
        if readiness.generation != self.provider_generation
            || readiness.state != ProviderInputReadiness::Ready
            || state
                .interactions
                .provider_input_state(&self.session_id)
                .await
                .is_none_or(|current| {
                    current.generation != self.provider_generation
                        || current.state != ProviderInputReadiness::Ready
                })
        {
            self.codex_attachment_ready.store(false, Ordering::Release);
            return Err("Codex readiness publication became stale".into());
        }
        assert_live_codex_incarnation(
            state,
            &self.session_id,
            self.runtime_generation,
            &self.config_lock,
        )
        .await?;
        let mut promotion_generation = Some(self.runtime_generation);
        let app = self.app.clone();
        let session_id = self.session_id.clone();
        let current_status = self.current_status.clone();
        complete_codex_attachment(
            &mut promotion_generation,
            self.runtime_generation,
            &self.codex_attachment_ready,
            &self.codex_reader_alive,
            |_| Ok(()),
            || super::set_agent_status(&app, &session_id, &current_status, "Idle"),
        )?;
        assert_live_codex_incarnation(
            state,
            &self.session_id,
            self.runtime_generation,
            &self.config_lock,
        )
        .await?;
        if let Some(guard) = self.cleanup_guard.as_mut() {
            guard.attached();
        }
        self.cleanup_guard.take();
        Ok(())
    }

    async fn fail_with_rollback(mut self, error: String) -> Result<(), String> {
        self.codex_attachment_ready.store(false, Ordering::Release);
        let owner_cleanup = self.dispose_owner().await.err();
        let registration = self.stop_registration.take();
        let rollback = crate::commands::agent::rollback_provisional_codex(
            &self.app,
            &self.session_id,
            self.provider_generation,
            self.runtime_generation,
            registration,
            &error,
        )
        .await
        .err();
        let mut message = error;
        if let Some(cleanup) = owner_cleanup {
            message.push_str(&format!("; native Codex cleanup retained: {cleanup}"));
        }
        if let Some(rollback) = rollback {
            message.push_str(&format!("; provisional rollback failed: {rollback}"));
        }
        Err(message)
    }

    async fn dispose_owner(&mut self) -> Result<(), String> {
        if let Some(guard) = self.cleanup_guard.take() {
            guard.dispose().await
        } else {
            self.native_delivery
                .dispose_codex_generation(&self.session_id, self.provider_generation)
                .await
                .map_err(|error| error.to_string())
        }
    }

    pub(crate) async fn cancel(mut self) -> Result<(), String> {
        let owner_cleanup = self.dispose_owner().await.err();
        let rollback = cancellation_requires_rollback(self.stop_registration.is_some())
            .then(|| {
                self.stop_registration
                    .take()
                    .expect("checked stop registration")
            })
            .map(|registration| async {
                crate::commands::agent::rollback_provisional_codex(
                    &self.app,
                    &self.session_id,
                    self.provider_generation,
                    self.runtime_generation,
                    Some(registration),
                    "Codex onboarding cancelled",
                )
                .await
            });
        let rollback_error = match rollback {
            Some(rollback) => rollback.await.err(),
            None => None,
        };
        match (owner_cleanup, rollback_error) {
            (None, None) => Ok(()),
            (owner, rollback) => {
                let mut message = "Codex onboarding cancellation cleanup retained".to_string();
                if let Some(error) = owner {
                    message.push_str(&format!(": {error}"));
                }
                if let Some(error) = rollback {
                    message.push_str(&format!("; provisional rollback failed: {error}"));
                }
                Err(message)
            }
        }
    }
}

pub(super) struct SynchronousCodexFinalizationContext<'a> {
    pub(super) native_delivery: &'a Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    pub(super) session_id: &'a str,
    pub(super) provider_generation: u64,
    pub(super) child: &'a mut super::codex_shared::StartingCodexTui,
    pub(super) config_lock: &'a Arc<std::sync::Mutex<AgentConfig>>,
    pub(super) codex_attachment_ready: &'a Arc<AtomicBool>,
    pub(super) codex_reader_alive: &'a Arc<AtomicBool>,
    pub(super) watch_state: &'a Arc<std::sync::Mutex<AgentWatchState>>,
}

pub(super) async fn finalize_synchronous_codex(
    context: SynchronousCodexFinalizationContext<'_>,
) -> Result<AgentConfig, String> {
    let SynchronousCodexFinalizationContext {
        native_delivery,
        session_id,
        provider_generation,
        child,
        config_lock,
        codex_attachment_ready,
        codex_reader_alive,
        watch_state,
    } = context;
    let finalized = native_delivery
        .finalize_codex_tui(
            session_id,
            provider_generation,
            || {
                let result = if !codex_reader_alive.load(Ordering::Acquire) {
                    Err(
                        crate::delivery::codex_shared::CodexSharedError::unsupported(
                            "captured Codex PTY reader exited during attachment",
                        ),
                    )
                } else {
                    child.alive()
                };
                async move { result }
            },
            |id| {
                let mut captured_config = config_lock.lock().map_err(|_| {
                    crate::delivery::codex_shared::CodexSharedError::unsupported(
                        "agent config lock unavailable",
                    )
                })?;
                super::apply_provider_identity("codex", &mut captured_config, id)
                    .map(|_| ())
                    .map_err(crate::delivery::codex_shared::CodexSharedError::unsupported)
            },
        )
        .await;
    if let Err(error) = finalized {
        child.stop().await;
        return Err(format_codex_attachment_error(
            error.to_string(),
            watch_state,
        ));
    }
    if let Err(error) = child.alive() {
        child.stop().await;
        return Err(error.to_string());
    }
    let config = config_lock
        .lock()
        .map_err(|_| "agent config lock unavailable".to_string())?
        .clone();
    codex_attachment_ready.store(true, Ordering::Release);
    if !codex_reader_alive.load(Ordering::Acquire) {
        codex_attachment_ready.store(false, Ordering::Release);
        child.stop().await;
        return Err("captured Codex PTY reader exited during finalization".into());
    }
    Ok(config)
}

pub(crate) fn format_codex_attachment_error(
    error: impl Into<String>,
    watch_state: &Arc<std::sync::Mutex<AgentWatchState>>,
) -> String {
    let error = error.into();
    let output = watch_state
        .lock()
        .ok()
        .and_then(|state| state.snapshot_since(None, Some(4096)).ok())
        .map(|snapshot| snapshot.output.text)
        .unwrap_or_default();
    if output.trim().is_empty() {
        error
    } else {
        format!("{error}\nProvider terminal output:\n{output}")
    }
}

async fn persist_codex_identity(
    state: &AppState,
    session_id: &str,
    runtime_generation: u64,
    config_lock: &Arc<std::sync::Mutex<AgentConfig>>,
) -> Result<(), String> {
    loop {
        let (barrier_busy, result) = {
            let agents = state.agents.lock().await;
            let order = state.agent_order.lock().await;
            match wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                .map_err(|error| error.to_string())?
            {
                Some(_barrier) => {
                    let agent = agents
                        .get(session_id)
                        .filter(|agent| agent.runtime_generation == Some(runtime_generation))
                        .filter(|agent| Arc::ptr_eq(&agent.config, config_lock))
                        .ok_or_else(|| {
                            "Codex attachment became stale before durable promotion".to_string()
                        });
                    let result = agent.and_then(|_| {
                        let snapshot = super::state_configs_snapshot(&agents, &order);
                        super::try_save_state_snapshot_unlocked(&snapshot)
                    });
                    (false, result)
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

async fn assert_live_codex_incarnation(
    state: &AppState,
    session_id: &str,
    runtime_generation: u64,
    config_lock: &Arc<std::sync::Mutex<AgentConfig>>,
) -> Result<(), String> {
    let mut agents = state.agents.lock().await;
    let agent = agents
        .get_mut(session_id)
        .filter(|agent| {
            exact_runtime_generation_matches(agent.runtime_generation, runtime_generation)
        })
        .filter(|agent| Arc::ptr_eq(&agent.config, config_lock))
        .ok_or_else(|| "Codex attachment became stale during promotion".to_string())?;
    let child = agent
        .child_process
        .as_mut()
        .ok_or_else(|| "Codex attachment child disappeared during promotion".to_string())?;
    match child.try_wait() {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err("captured ordinary Codex TUI exited during promotion".into()),
        Err(error) => Err(format!(
            "could not inspect captured Codex TUI during promotion: {error}"
        )),
    }
}

pub(crate) fn should_publish_provisionally(provider: &str, is_restored: bool) -> bool {
    provider == "codex" && !is_restored
}

pub(crate) fn complete_codex_attachment<F, G>(
    current_runtime_generation: &mut Option<u64>,
    expected_runtime_generation: u64,
    attachment_ready: &AtomicBool,
    reader_alive: &AtomicBool,
    finalize: F,
    publish_ready: G,
) -> Result<(), String>
where
    F: FnOnce(&mut Option<u64>) -> Result<(), String>,
    G: FnOnce(),
{
    attachment_ready.store(false, Ordering::Release);
    if !exact_runtime_generation_matches(*current_runtime_generation, expected_runtime_generation) {
        return Err("Codex attachment generation is stale before promotion".into());
    }
    finalize(current_runtime_generation)?;
    if !exact_runtime_generation_matches(*current_runtime_generation, expected_runtime_generation) {
        return Err("Codex attachment generation is stale after finalization".into());
    }
    attachment_ready.store(true, Ordering::Release);
    if !reader_alive.load(Ordering::Acquire) {
        attachment_ready.store(false, Ordering::Release);
        return Err("captured Codex PTY reader exited during finalization".into());
    }
    if !exact_runtime_generation_matches(*current_runtime_generation, expected_runtime_generation) {
        attachment_ready.store(false, Ordering::Release);
        return Err("Codex attachment generation is stale during promotion".into());
    }
    publish_ready();
    if !reader_alive.load(Ordering::Acquire) {
        attachment_ready.store(false, Ordering::Release);
        return Err("captured Codex PTY reader exited during readiness publication".into());
    }
    if !exact_runtime_generation_matches(*current_runtime_generation, expected_runtime_generation) {
        attachment_ready.store(false, Ordering::Release);
        return Err("Codex attachment generation is stale after readiness publication".into());
    }
    Ok(())
}

pub(crate) fn exact_runtime_generation_matches(
    current_generation: Option<u64>,
    expected_generation: u64,
) -> bool {
    current_generation == Some(expected_generation)
}

pub(crate) fn cancellation_requires_rollback(stop_registration_present: bool) -> bool {
    stop_registration_present
}

pub(crate) fn format_cleanup_errors(
    failure: &str,
    roster: Option<String>,
    terminal: Option<String>,
    stop: Option<String>,
) -> String {
    let mut message = failure.to_string();
    for (label, error) in [("roster", roster), ("terminal", terminal), ("Codex", stop)] {
        if let Some(error) = error {
            message.push_str(&format!("; {label} cleanup retained: {error}"));
        }
    }
    message
}
