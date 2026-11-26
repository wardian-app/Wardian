use super::{renew_agent_lifecycle_transition_lease, LIFECYCLE_LEASE_DURATION};
use crate::manager;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

const LIFECYCLE_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// Keeps a long lifecycle operation from outliving the persisted exclusion
/// lease that protects it. If renewal fails, callers must stop before the next
/// destructive or replacement step rather than continuing without ownership.
type LifecycleLeaseRenewal =
    fn(&wardian_core::conversation_lease::ConversationLeaseOwner) -> Result<bool, String>;

pub(crate) struct LifecycleLeaseHeartbeat {
    owner: wardian_core::conversation_lease::ConversationLeaseOwner,
    active: Arc<AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    renew: LifecycleLeaseRenewal,
}

impl LifecycleLeaseHeartbeat {
    pub(crate) fn owner(&self) -> &wardian_core::conversation_lease::ConversationLeaseOwner {
        &self.owner
    }

    pub(super) fn start(owner: wardian_core::conversation_lease::ConversationLeaseOwner) -> Self {
        Self::start_with(owner, |owner| {
            renew_agent_lifecycle_transition_lease(owner, chrono::Utc::now())
        })
    }

    pub(crate) fn start_codex_recovery(
        owner: wardian_core::conversation_lease::ConversationLeaseOwner,
    ) -> Self {
        Self::start_with(owner, |owner| {
            use wardian_core::conversation_lease::{
                refresh_codex_recovery_lease_persisted, CodexRecoveryLeaseScope,
            };
            refresh_codex_recovery_lease_persisted(
                owner,
                LIFECYCLE_LEASE_DURATION,
                CodexRecoveryLeaseScope::Current,
            )
        })
    }

    fn start_with(
        owner: wardian_core::conversation_lease::ConversationLeaseOwner,
        renew: LifecycleLeaseRenewal,
    ) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let active_for_task = Arc::clone(&active);
        let owner_for_task = owner.clone();
        let (shutdown, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let first_heartbeat = tokio::time::Instant::now() + LIFECYCLE_LEASE_HEARTBEAT_INTERVAL;
        let task = tokio::spawn(async move {
            let mut heartbeat =
                tokio::time::interval_at(first_heartbeat, LIFECYCLE_LEASE_HEARTBEAT_INTERVAL);

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = heartbeat.tick() => {
                        match renew(&owner_for_task) {
                            Ok(true) => {}
                            Ok(false) => {
                                active_for_task.store(false, Ordering::Release);
                                manager::log_debug(&format!(
                                    "[WARDIAN] lifecycle lease was lost before renewal for {} {}",
                                    owner_for_task.owner_kind, owner_for_task.owner_id
                                ));
                                break;
                            }
                            Err(error) => {
                                active_for_task.store(false, Ordering::Release);
                                manager::log_debug(&format!(
                                    "[WARDIAN] lifecycle lease renewal failed for {} {}: {error}",
                                    owner_for_task.owner_kind, owner_for_task.owner_id
                                ));
                                break;
                            }
                        }
                    }
                }
            }
        });

        Self {
            owner,
            active,
            shutdown: Some(shutdown),
            task,
            renew,
        }
    }

    pub(super) fn ensure_active(&self, operation: &str) -> Result<(), String> {
        self.ensure_active_with(operation, || (self.renew)(&self.owner))
    }

    pub(crate) fn ensure_codex_recovery_scope(
        &self,
        operation: &str,
        scope: wardian_core::conversation_lease::CodexRecoveryLeaseScope<'_>,
    ) -> Result<(), String> {
        self.ensure_active_with(operation, || {
            wardian_core::conversation_lease::refresh_codex_recovery_lease_persisted(
                &self.owner,
                LIFECYCLE_LEASE_DURATION,
                scope,
            )
        })
    }

    /// Renew then fence a synchronous launch effect against replacement.
    pub(crate) fn with_exact_codex_recovery_effect<T>(
        &self,
        operation: &str,
        agent_id: &str,
        native_id: &str,
        effect: impl FnOnce() -> T,
    ) -> Result<T, String> {
        use wardian_core::conversation_lease::CodexRecoveryLeaseScope;
        self.ensure_codex_recovery_scope(
            operation,
            CodexRecoveryLeaseScope::Exact {
                agent_id,
                native_id,
            },
        )?;
        self.with_current_codex_recovery_lease(operation, agent_id, native_id, effect)
    }

    pub(crate) fn with_current_codex_recovery_lease<T>(
        &self,
        operation: &str,
        agent_id: &str,
        native_id: &str,
        effect: impl FnOnce() -> T,
    ) -> Result<T, String> {
        use wardian_core::conversation_lease::with_exact_codex_recovery_lease;
        if !self.active.load(Ordering::Acquire) {
            return Err(format!(
                "cannot {operation} agent because its lifecycle conversation lease was lost"
            ));
        }
        match with_exact_codex_recovery_lease(&self.owner, agent_id, native_id, |_| effect()) {
            Ok(Some(result)) => Ok(result),
            Ok(None) | Err(_) => {
                self.active.store(false, Ordering::Release);
                Err(format!(
                    "cannot {operation} agent because its lifecycle conversation lease was lost"
                ))
            }
        }
    }

    fn ensure_active_with(
        &self,
        operation: &str,
        renew: impl FnOnce() -> Result<bool, String>,
    ) -> Result<(), String> {
        if !self.active.load(Ordering::Acquire) {
            return Err(format!(
                "cannot {operation} agent because its lifecycle conversation lease was lost"
            ));
        }

        // A periodic heartbeat is not enough at a destructive boundary: renew
        // synchronously so this exact acquisition remains fenced for the next
        // lifecycle mutation even if a background renewal failed moments ago.
        match renew() {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => {
                self.active.store(false, Ordering::Release);
                Err(format!(
                    "cannot {operation} agent because its lifecycle conversation lease was lost"
                ))
            }
        }
    }

    pub(super) async fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

impl Drop for LifecycleLeaseHeartbeat {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}
