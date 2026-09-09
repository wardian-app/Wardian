//! Per-agent lifecycle exclusion and awaited native-owner shutdown.
use crate::state::AppState;

#[cfg(test)]
#[path = "agent/lifecycle_tests.rs"]
mod tests;

pub(super) async fn lock_agent_lifecycle(
    state: &AppState,
    session_id: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    state.lock_agent_lifecycle(session_id).await
}

pub(super) async fn acquire_agent_lifecycle_guard(
    state: &AppState,
    session_id: &str,
    existing: Option<tokio::sync::OwnedMutexGuard<()>>,
) -> tokio::sync::OwnedMutexGuard<()> {
    match existing {
        Some(guard) => guard,
        None => lock_agent_lifecycle(state, session_id).await,
    }
}

/// Caller already holds lifecycle exclusion. A failed stop must block the
/// pause/replacement rather than release the registry's still-owned process.
pub(super) async fn stop_native_owner(
    state: &AppState,
    session_id: &str,
    remove_terminal: bool,
) -> Result<(), String> {
    state
        .native_delivery
        .dispose_agent(session_id)
        .await
        .map_err(|error| error.to_string())?;
    stop_codex_runtime(state, session_id, remove_terminal).await
}

/// Explicit lifecycle operations may retry the same retained children. Native
/// startup only awaits quiescence; it must never retry a failed stop itself.
/// Finish this before clear/resume bootstrap, even when the roster looks Off.
async fn stop_codex_runtime(
    state: &AppState,
    session_id: &str,
    remove_terminal: bool,
) -> Result<(), String> {
    let config = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .ok_or_else(|| format!("Agent {session_id} not found"))?
            .config
            .clone()
    };
    let is_codex = config
        .lock()
        .map_err(|_| "Agent configuration lock poisoned")?
        .provider
        == "codex";
    if !is_codex {
        return Ok(());
    }
    let home = crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
    crate::manager::codex_stop::retry_stop(&home, session_id)?;
    crate::manager::codex_stop::await_quiescent(&home, session_id).await?;
    // Validate while the roster still owns the runtime. Lifecycle exclusion
    // covers the interval until capture; this token does not fence owner startup.
    let registration = crate::manager::codex_stop::prepare_stop(&home, session_id)?;
    let (guard, generation) = {
        let mut agents = state.agents.lock().await;
        let agent = agents
            .get_mut(session_id)
            .ok_or_else(|| format!("Agent {session_id} not found"))?;
        let generation = agent.runtime_generation;
        let guard = registration.capture(super::take_agent_runtime_for_termination(agent));
        (guard, generation)
    };
    // From capture onward neither cancellation nor a later rollback can restore
    // an unjoined runtime. Keep config/status in the roster; the fence, rather
    // than empty fields or Off, is the authority for subsequent startup.
    if let Some(generation) = generation {
        let result = if remove_terminal {
            state
                .terminal_sessions
                .terminate_and_remove_runtime(session_id, generation)
                .await
        } else {
            state
                .terminal_sessions
                .pause_runtime(session_id, generation)
                .await
                .map(|_| ())
        };
        if let Err(error) = result {
            crate::manager::log_debug(&format!(
                "[WARDIAN] Codex terminal shutdown for {session_id}: {error}"
            ));
        }
    }
    // No roster/config/order/preparation lock is held by this waiter. The guard
    // also starts the retained worker if the broker await above is cancelled.
    guard.begin_stop().wait().await
}

/// Finite launch/registration guard: uncommitted Codex runtimes transfer their
/// child handles and exact terminal generation into retained cancellation cleanup.
/// Installed ActiveAgent Drop is unchanged.
pub(super) struct PendingRuntime {
    runtime: Option<crate::state::ActiveAgent>,
    registration: Option<crate::manager::codex_stop::StopRegistration>,
}

impl PendingRuntime {
    /// Validate cleanup before spawn; the token itself does not block startup.
    pub(super) fn prepare(
        config: &wardian_core::models::AgentConfig,
        terminal_sessions: &std::sync::Arc<crate::state::terminal_session::TerminalSessionBroker>,
    ) -> Result<Self, String> {
        let registration = if config.provider == "codex" {
            let home =
                crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
            Some(
                crate::manager::codex_stop::prepare_stop(&home, &config.session_id)?
                    .with_terminal_cleanup(terminal_sessions.clone())?,
            )
        } else {
            None
        };
        Ok(Self {
            runtime: None,
            registration,
        })
    }

    /// Call in the same expression that receives a successful spawn result.
    pub(super) fn attach(mut self, runtime: crate::state::ActiveAgent) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Only synchronous commit code may take this slot and install the runtime.
    pub(super) fn slot(&mut self) -> &mut Option<crate::state::ActiveAgent> {
        &mut self.runtime
    }

    pub(super) fn installed(mut self) -> crate::state::ActiveAgent {
        self.runtime.take().expect("pending runtime installed once")
    }

    pub(super) fn begin_stop(mut self) -> PendingStop {
        match self.registration.take() {
            Some(registration) => PendingStop(Some(
                registration
                    .capture(self.runtime.take().expect("pending runtime stopped once"))
                    .begin_stop(),
            )),
            None => {
                if let Some(mut runtime) = self.runtime.take() {
                    crate::manager::terminate_active_agent_process(&mut runtime);
                }
                PendingStop(None)
            }
        }
    }

    pub(super) async fn stop_after_failure(self, error: String) -> String {
        self.begin_stop().failure(error).await
    }
}

impl std::ops::Deref for PendingRuntime {
    type Target = crate::state::ActiveAgent;
    fn deref(&self) -> &Self::Target {
        self.runtime.as_ref().expect("pending runtime")
    }
}

impl std::ops::DerefMut for PendingRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.runtime.as_mut().expect("pending runtime")
    }
}

impl Drop for PendingRuntime {
    fn drop(&mut self) {
        if self.runtime.is_some() {
            if let Some(registration) = self.registration.take() {
                // No await or fallible operation between take and registry capture.
                drop(registration.capture(self.runtime.take().expect("checked runtime")));
            }
            // Other providers retain ActiveAgent's existing Drop behavior.
        }
    }
}

pub(super) struct PendingStop(Option<crate::manager::codex_stop::StopHandle>);

impl PendingStop {
    pub(super) async fn failure(self, error: String) -> String {
        if let Some(handle) = self.0 {
            if let Err(stop_error) = handle.wait().await {
                return format!("{error}; Codex cleanup retained: {stop_error}");
            }
        }
        error
    }
}
