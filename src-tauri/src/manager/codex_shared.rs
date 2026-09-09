//! A failed PTY attachment must release only its own provider generation.
use crate::delivery::native_broker::NativeDeliveryBroker;
use std::sync::Arc;

pub(crate) fn background_starts_fresh(config: &wardian_core::models::AgentConfig) -> bool {
    use wardian_core::models::{AgentSessionPersistence, AgentSessionPersistenceOverride};
    match config.session_persistence {
        AgentSessionPersistenceOverride::Fresh => true,
        AgentSessionPersistenceOverride::Resume => false,
        AgentSessionPersistenceOverride::Default => {
            crate::utils::load_shell_settings()
                .unwrap_or_default()
                .agent_session_persistence
                == AgentSessionPersistence::Fresh
        }
    }
}

fn captured_background_config(
    current: &wardian_core::models::AgentConfig,
    expected: &wardian_core::models::AgentConfig,
    native_id: &str,
    started_fresh: bool,
) -> Result<wardian_core::models::AgentConfig, String> {
    if current.provider != "codex"
        || !current.is_off
        || current.session_id != expected.session_id
        || current.resume_session != expected.resume_session
        || current.session_persistence != expected.session_persistence
    {
        return Err("background session publication no longer matches the prepared agent".into());
    }
    let mut updated = current.clone();
    if started_fresh {
        updated.resume_session = None;
    }
    super::apply_provider_identity("codex", &mut updated, native_id)?;
    Ok(updated)
}

/// Publish a just-created/confirmed native session through the ordinary atomic
/// agent-state snapshot, never by reinterpreting a historical diagnostic. Caller
/// retains the exact background lease; this helper takes the short local
/// lifecycle guard only for publication, after long initialization has finished.
pub(crate) async fn publish_background_identity(
    state: &crate::state::AppState,
    spec: &crate::delivery::native_broker::NativeSessionSpec,
    native_id: &str,
    started_fresh: bool,
) -> Result<(), String> {
    let _lifecycle = state.lock_agent_lifecycle(&spec.target_agent_id).await;
    if state
        .interactions
        .current_provider_input_generation(&spec.target_agent_id)
        .await
        != Some(spec.generation)
    {
        return Err("background identity generation is no longer current".into());
    }
    let _barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .map_err(|error| error.to_string())?
        .ok_or("agent roster barrier unavailable")?;
    let agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    let agent = agents
        .get(&spec.target_agent_id)
        .ok_or("background target was removed")?;
    let current = agent
        .config
        .lock()
        .map_err(|_| "agent config lock unavailable")?
        .clone();
    let updated = captured_background_config(&current, &spec.config, native_id, started_fresh)?;
    let mut snapshot = super::state_configs_snapshot(&agents, &order);
    let entry = snapshot
        .iter_mut()
        .find(|config| config.session_id == spec.target_agent_id)
        .ok_or("background target missing from agent order")?;
    *entry = updated.clone();
    super::try_save_state_snapshot_unlocked(&snapshot)?;
    *agent
        .config
        .lock()
        .map_err(|_| "agent config lock unavailable")? = updated;
    Ok(())
}

/// Publish TUI- as well as broker-originated turns through the ordinary runtime
/// status path. That path fences persistence/UI emission by the status Arc of
/// the current incarnation. Mere inbox appends produce no activity transition.
pub(super) fn observe_turn_activity(
    app: tauri::AppHandle,
    agent_id: String,
    current_status: Arc<std::sync::Mutex<String>>,
    mut observations: tokio::sync::watch::Receiver<crate::delivery::codex_shared::Observation>,
) {
    use crate::delivery::codex_shared::CodexTurnActivity;
    tauri::async_runtime::spawn(async move {
        let mut previous = CodexTurnActivity::Pending;
        loop {
            let activity = observations.borrow_and_update().activity();
            if current_status.lock().is_ok_and(|status| *status == "Off") {
                break;
            }
            if activity != previous {
                match &activity {
                    CodexTurnActivity::Processing(_) => {
                        super::set_agent_status(&app, &agent_id, &current_status, "Processing...");
                    }
                    CodexTurnActivity::Idle(_) => {
                        super::apply_agent_status_event(
                            &app,
                            &agent_id,
                            wardian_core::models::provider::AgentEvent::TurnCompleted,
                            &current_status,
                        );
                    }
                    CodexTurnActivity::Closed => {
                        super::set_agent_status(&app, &agent_id, &current_status, "Error");
                        break;
                    }
                    CodexTurnActivity::Stopped => break,
                    CodexTurnActivity::Pending => {}
                }
                previous = activity;
            }
            if observations.changed().await.is_err() {
                break;
            }
        }
    });
}

pub(super) struct CodexAttachGuard {
    broker: Arc<NativeDeliveryBroker>,
    agent_id: String,
    generation: u64,
    armed: bool,
    reader_state: Option<(
        Arc<std::sync::atomic::AtomicBool>,
        Arc<std::sync::atomic::AtomicBool>,
    )>,
}

/// Own the exact starting PTY before any fallible reader/broker setup. Cleanup
/// survives cancellation and joins this child before releasing its owner slot.
pub(super) struct StartingCodexTui {
    child: Option<Box<dyn portable_pty::Child + Send>>,
    owner: Option<(Arc<NativeDeliveryBroker>, String, u64)>,
    runtime: Option<(
        Arc<crate::state::terminal_session::TerminalSessionBroker>,
        u64,
    )>,
}

impl StartingCodexTui {
    pub(super) fn new(
        child: Box<dyn portable_pty::Child + Send>,
        owner: Option<(Arc<NativeDeliveryBroker>, String, u64)>,
    ) -> Self {
        Self {
            child: Some(child),
            owner,
            runtime: None,
        }
    }

    pub(super) fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.process_id())
    }

    pub(super) fn alive(&mut self) -> Result<(), crate::delivery::codex_shared::CodexSharedError> {
        use crate::delivery::codex_shared::CodexSharedError;
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| CodexSharedError::unsupported("captured TUI missing"))?;
        match child.try_wait() {
            Ok(None) => Ok(()),
            _ => Err(CodexSharedError::unsupported(
                "captured ordinary Codex TUI exited during attachment",
            )),
        }
    }

    pub(super) fn runtime(
        &mut self,
        broker: Arc<crate::state::terminal_session::TerminalSessionBroker>,
        generation: u64,
    ) {
        if self.owner.is_some() {
            self.runtime = Some((broker, generation));
        }
    }

    pub(super) fn attached(mut self) -> Box<dyn portable_pty::Child + Send> {
        self.owner = None;
        self.runtime = None;
        self.child
            .take()
            .expect("starting PTY child retained until attachment")
    }

    fn cleanup(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let (broker, agent_id, generation) = self.owner.take()?;
        let mut child = self.child.take()?;
        let runtime = self.runtime.take();
        Some(tokio::spawn(async move {
            // The detached cleanup task retains ownership even if spawn's
            // caller disappears while waiting for the blocking PTY child.
            let stopped = tokio::task::spawn_blocking(move || loop {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                #[cfg(windows)]
                if let Some(pid) = child.process_id() {
                    let _ = crate::utils::process::force_kill_process_tree(pid);
                }
                let _ = child.kill();
                if child.wait().is_ok() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            })
            .await;
            if stopped.is_err() {
                // No observed exit: retain the owner slot, preventing replacement.
                crate::utils::logging::log_debug("Codex TUI cleanup ended without observed exit");
                return;
            }
            if let Some((terminal, runtime_generation)) = runtime {
                let _ = terminal
                    .terminate_runtime(&agent_id, runtime_generation)
                    .await;
            }
            if let Err(error) = broker.dispose_codex_generation(&agent_id, generation).await {
                crate::utils::logging::log_debug(&format!("Codex captured TUI cleanup: {error}"));
            }
        }))
    }

    pub(super) async fn stop(&mut self) {
        if let Some(cleanup) = self.cleanup() {
            let _ = cleanup.await;
        }
    }
}

impl Drop for StartingCodexTui {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

impl CodexAttachGuard {
    pub(super) fn new(
        broker: Arc<NativeDeliveryBroker>,
        agent_id: String,
        generation: u64,
    ) -> Self {
        Self {
            broker,
            agent_id,
            generation,
            armed: true,
            reader_state: None,
        }
    }

    pub(super) fn attached(&mut self) {
        self.armed = false;
    }

    pub(super) fn for_reader(
        mut self,
        alive: Arc<std::sync::atomic::AtomicBool>,
        attachment_ready: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.reader_state = Some((alive, attachment_ready));
        self
    }
}

impl Drop for CodexAttachGuard {
    fn drop(&mut self) {
        if let Some((alive, ready)) = &self.reader_state {
            alive.store(false, std::sync::atomic::Ordering::Release);
            if !ready.swap(false, std::sync::atomic::Ordering::AcqRel) {
                // Before attachment, StartingCodexTui must join the captured
                // child before releasing the owner slot. Report EOF to its gate.
                return;
            }
        }
        if !self.armed {
            return;
        }
        let broker = self.broker.clone();
        let agent_id = self.agent_id.clone();
        let generation = self.generation;
        tauri::async_runtime::spawn(async move {
            if let Err(error) = broker.dispose_codex_generation(&agent_id, generation).await {
                crate::utils::logging::log_debug(&format!("Codex failed-attach cleanup: {error}"));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::{AgentConfig, AgentSessionPersistenceOverride};

    #[test]
    fn background_native_capture_retains_resume_continuity_and_explicit_fresh() {
        let first_id = "01a07df1-8c19-7fe1-8b01-905a97a42eef";
        let next_id = "01a07df1-8c19-7fe1-8b01-905a97a42eee";
        let initial = AgentConfig {
            session_id: "wardian-owner".into(),
            provider: "codex".into(),
            is_off: true,
            model: Some("explicit-model".into()),
            session_persistence: AgentSessionPersistenceOverride::Resume,
            ..Default::default()
        };
        let captured = captured_background_config(&initial, &initial, first_id, false).unwrap();
        assert_eq!(captured.resume_session.as_deref(), Some(first_id));
        assert_eq!(captured.model, initial.model);
        assert!(captured.is_off);
        assert!(captured_background_config(&captured, &captured, first_id, false).is_ok());
        assert!(captured_background_config(&captured, &captured, next_id, false).is_err());
        let mut fresh = captured.clone();
        fresh.session_persistence = AgentSessionPersistenceOverride::Fresh;
        let next = captured_background_config(&fresh, &fresh, next_id, true).unwrap();
        assert_eq!(next.resume_session.as_deref(), Some(next_id));
        assert_eq!(
            next.session_persistence,
            AgentSessionPersistenceOverride::Fresh
        );
        assert!(background_starts_fresh(&next));
        // An old publication cannot overwrite a changed resume target/clear.
        assert!(captured_background_config(&captured, &initial, first_id, false).is_err());
        assert!(
            captured_background_config(&initial, &initial, "invalid-native-id", false).is_err()
        );
    }
}
