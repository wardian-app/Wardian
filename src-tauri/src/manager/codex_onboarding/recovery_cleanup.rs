use super::{
    AgentConfig, AppHandle, Arc, AtomicBool, CodexAttachGuard, Manager, Ordering, Runtime,
};
use crate::manager::{codex_stop::StopRegistration, RegistrationPublicationState};

pub(crate) struct CapturedRecoveryRuntime {
    pub(crate) session_id: String,
    pub(crate) provider_generation: u64,
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
            provider_generation: completion.provider_generation,
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
    capture: Option<(
        CapturedRecoveryRuntime,
        StopRegistration,
        Option<CodexAttachGuard>,
    )>,
}

impl<R: Runtime> RecoveryRuntimeCleanup<R> {
    pub(super) fn new(
        app: AppHandle<R>,
        identity: CapturedRecoveryRuntime,
        registration: StopRegistration,
        owner: Option<CodexAttachGuard>,
    ) -> Self {
        Self {
            app,
            capture: Some((identity, registration, owner)),
        }
    }

    pub(crate) fn disarm(mut self) {
        if let Some((identity, _, owner)) = self.capture.take() {
            if let Some(mut owner) = owner {
                owner.attached();
            }
            identity.publication.recovery_activated();
        }
    }

    pub(crate) async fn finish(mut self, mut failure: String) -> String {
        if let Some(task) = self.schedule(failure.clone()) {
            match task.await {
                Ok(cleanup) => return cleanup,
                Err(error) => failure.push_str(&format!("; captured cleanup task failed: {error}")),
            }
        }
        failure
    }

    fn schedule(
        &mut self,
        mut failure: String,
    ) -> Option<tauri::async_runtime::JoinHandle<String>> {
        let (identity, registration, owner) = self.capture.take()?;
        identity.publication.begin_recovery_cleanup();
        identity.ready.store(false, Ordering::Release);
        let app = self.app.clone();
        Some(tauri::async_runtime::spawn(async move {
            if let Err(error) =
                crate::commands::agent::rollback_captured_recovery(&app, &identity, registration)
                    .await
            {
                failure.push_str(&format!("; captured TUI rollback failed: {error}"));
            }
            let native = match owner {
                Some(owner) => owner.dispose().await,
                None => app
                    .state::<crate::state::AppState>()
                    .native_delivery
                    .dispose_codex_generation(&identity.session_id, identity.provider_generation)
                    .await
                    .map_err(|error| error.to_string()),
            };
            if let Err(error) = native {
                failure.push_str(&format!("; native Codex cleanup retained: {error}"));
            }
            failure
        }))
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        app: AppHandle<R>,
        active: &crate::state::ActiveAgent,
        provider_generation: u64,
        publication: Arc<RegistrationPublicationState>,
    ) -> Self {
        let session_id = active.config.lock().unwrap().session_id.clone();
        let state = app.state::<crate::state::AppState>();
        let stop = crate::manager::codex_stop::prepare_stop(
            &crate::utils::fs::get_wardian_home().unwrap(),
            &session_id,
        )
        .unwrap()
        .with_terminal_cleanup(state.terminal_sessions.clone())
        .unwrap();
        let owner = CodexAttachGuard::new(
            state.native_delivery.clone(),
            session_id.clone(),
            provider_generation,
        );
        Self::new(
            app,
            CapturedRecoveryRuntime {
                session_id,
                provider_generation,
                runtime_generation: active.runtime_generation.unwrap(),
                config: active.config.clone(),
                publication,
                ready: active
                    .watch_state
                    .lock()
                    .unwrap()
                    .codex_attachment_ready_flag(),
            },
            stop,
            Some(owner),
        )
    }
}

impl<R: Runtime> Drop for RecoveryRuntimeCleanup<R> {
    fn drop(&mut self) {
        if let Some(task) = self.schedule("Codex recovery activation was abandoned".into()) {
            tauri::async_runtime::spawn(async move {
                if let Ok(message) = task.await {
                    crate::manager::log_debug(&format!("[WARDIAN] {message}"));
                }
            });
        }
    }
}
