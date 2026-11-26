use super::{AgentConfig, AppHandle, Arc, AtomicBool, Ordering, Runtime};
use crate::manager::{codex_stop::StopRegistration, RegistrationPublicationState};
#[cfg(test)]
use tauri::Manager;

pub(crate) struct CapturedRecoveryRuntime {
    pub(crate) session_id: String,
    pub(crate) runtime_generation: u64,
    pub(crate) config: Arc<std::sync::Mutex<AgentConfig>>,
    pub(crate) publication: Arc<RegistrationPublicationState>,
    ready: Arc<AtomicBool>,
}

impl CapturedRecoveryRuntime {
    pub(super) fn from_completion(
        completion: &super::CodexAttachmentCompletion,
        publication: Arc<RegistrationPublicationState>,
    ) -> Self {
        Self {
            session_id: completion.session_id.clone(),
            runtime_generation: completion.runtime_generation,
            config: completion.config_lock.clone(),
            publication,
            ready: completion.codex_attachment_ready.clone(),
        }
    }
}

/// Own both sides of the installed provisional runtime until input activation.
/// Dropping a pending worker schedules the same exact cleanup as a failure.
pub(crate) struct RecoveryRuntimeCleanup<R: Runtime = tauri::Wry> {
    app: AppHandle<R>,
    capture: Option<(CapturedRecoveryRuntime, StopRegistration)>,
}

impl<R: Runtime> RecoveryRuntimeCleanup<R> {
    pub(super) fn new(
        app: AppHandle<R>,
        identity: CapturedRecoveryRuntime,
        registration: StopRegistration,
    ) -> Self {
        Self {
            app,
            capture: Some((identity, registration)),
        }
    }

    pub(crate) fn disarm(mut self) {
        drop(self.capture.take());
    }

    pub(crate) async fn finish(self, failure: String) -> String {
        match self.cancel().await {
            Ok(()) => failure,
            Err(error) => format!("{failure}; captured Codex rollback failed: {error}"),
        }
    }

    pub(super) async fn cancel(mut self) -> Result<(), String> {
        if let Some(task) = self.schedule() {
            task.await
                .map_err(|error| format!("captured cleanup task failed: {error}"))?
        } else {
            Ok(())
        }
    }

    fn schedule(&mut self) -> Option<tauri::async_runtime::JoinHandle<Result<(), String>>> {
        let (identity, registration) = self.capture.take()?;
        identity.publication.begin_recovery_cleanup();
        identity.ready.store(false, Ordering::Release);
        let app = self.app.clone();
        Some(tauri::async_runtime::spawn(async move {
            crate::commands::agent::rollback_captured_recovery(&app, &identity, registration).await
        }))
    }
}

#[cfg(test)]
impl CapturedRecoveryRuntime {
    pub(crate) fn fixture(
        active: &crate::state::ActiveAgent,
        publication: Arc<RegistrationPublicationState>,
    ) -> Self {
        Self {
            session_id: active.config.lock().unwrap().session_id.clone(),
            runtime_generation: active.runtime_generation.unwrap(),
            config: active.config.clone(),
            publication,
            ready: active
                .watch_state
                .lock()
                .unwrap()
                .codex_attachment_ready_flag(),
        }
    }
}

#[cfg(test)]
impl<R: Runtime> RecoveryRuntimeCleanup<R> {
    pub(crate) fn fixture(
        app: AppHandle<R>,
        active: &crate::state::ActiveAgent,
        provider_generation: u64,
        publication: Arc<RegistrationPublicationState>,
    ) -> Self {
        let capture = CapturedRecoveryRuntime::fixture(active, publication);
        let state = app.state::<crate::state::AppState>();
        let stop = crate::manager::codex_stop::prepare_stop(
            &crate::utils::fs::get_wardian_home().unwrap(),
            &capture.session_id,
        )
        .unwrap()
        .with_terminal_cleanup(state.terminal_sessions.clone())
        .unwrap()
        .with_recovery_owner(state.native_delivery.clone(), provider_generation);
        Self::new(app, capture, stop)
    }
}

impl<R: Runtime> Drop for RecoveryRuntimeCleanup<R> {
    fn drop(&mut self) {
        if let Some(task) = self.schedule() {
            tauri::async_runtime::spawn(async move {
                let problem = task
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
                    .err();
                crate::manager::log_debug(&match problem {
                    Some(error) => format!("[WARDIAN] Codex recovery activation was abandoned; captured cleanup retained: {error}"),
                    None => "[WARDIAN] Codex recovery activation was abandoned".into(),
                });
            });
        }
    }
}
