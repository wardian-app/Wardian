use super::{RegistrationPublicationState, PROVIDER_SPAWN_LEASE_DURATION};
use std::sync::{Mutex, MutexGuard};
use tokio::sync::watch;
use wardian_core::conversation_lease::{
    refresh_codex_recovery_lease_persisted, with_exact_codex_recovery_lease,
    CodexRecoveryLeaseScope, ConversationLeaseOwner,
};

const LEASE_LOST: &str = "Codex recovery lost its exact registration lease";

pub(super) struct RecoveryPublication {
    owner: ConversationLeaseOwner,
    agent_id: String,
    native_id: String,
    // Serialize watcher loss, cancellation, and durable success. This lock
    // only covers synchronous persisted checks and the final SQLite commit.
    transition: Mutex<()>,
    lost: watch::Sender<bool>,
}

impl RecoveryPublication {
    pub(super) fn lock(&self) -> MutexGuard<'_, ()> {
        self.transition
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn with_lease<T>(&self, effect: impl FnOnce() -> T) -> Result<Option<T>, String> {
        with_exact_codex_recovery_lease(&self.owner, &self.agent_id, &self.native_id, |_| effect())
    }
}

impl RegistrationPublicationState {
    pub(crate) fn for_recovery(
        owner: ConversationLeaseOwner,
        agent_id: String,
        native_id: String,
    ) -> Self {
        Self {
            recovery: Some(RecoveryPublication {
                owner,
                agent_id,
                native_id,
                transition: Mutex::new(()),
                lost: watch::channel(false).0,
            }),
            ..Default::default()
        }
    }

    pub(crate) fn is_recovery(&self) -> bool {
        self.recovery.is_some()
    }

    pub(crate) fn recovery_lease_lost(&self) -> bool {
        self.recovery
            .as_ref()
            .is_some_and(|recovery| *recovery.lost.borrow())
    }

    pub(crate) async fn wait_for_recovery_lease_loss(&self) {
        if let Some(recovery) = &self.recovery {
            let _ = recovery.lost.subscribe().wait_for(|lost| *lost).await;
        }
    }

    fn record_loss(&self, recovery: &RecoveryPublication) -> String {
        self.set_failed();
        recovery.lost.send_replace(true);
        LEASE_LOST.into()
    }

    /// Gate a synchronous promotion while the lease file excludes replacements.
    pub(crate) fn with_recovery_publication<T>(
        &self,
        publish: impl FnOnce() -> T,
    ) -> Result<T, String> {
        let Some(recovery) = &self.recovery else {
            return Ok(publish());
        };
        let _transition = recovery.lock();
        if self.state() != Self::PENDING {
            return Err(if *recovery.lost.borrow() {
                LEASE_LOST
            } else {
                "Codex recovery registration is no longer pending"
            }
            .into());
        }
        match recovery.with_lease(publish) {
            Ok(Some(result)) => Ok(result),
            Ok(None) | Err(_) => Err(self.record_loss(recovery)),
        }
    }

    pub(crate) fn renew_recovery_lease(&self) -> Result<(), String> {
        let recovery = self
            .recovery
            .as_ref()
            .ok_or("Missing Codex recovery registration")?;
        let _transition = recovery.lock();
        if self.state() != Self::PENDING {
            // Successful publication intentionally permits the watcher to
            // release. A later missing lease is therefore not a loss.
            return Ok(());
        }
        match refresh_codex_recovery_lease_persisted(
            &recovery.owner,
            PROVIDER_SPAWN_LEASE_DURATION,
            CodexRecoveryLeaseScope::Exact {
                agent_id: &recovery.agent_id,
                native_id: &recovery.native_id,
            },
        ) {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(self.record_loss(recovery)),
        }
    }

    /// Only a successful durable commit allows the watcher to release.
    pub(crate) fn commit_recovery_registration(
        &self,
        operation_id: &str,
        attempt_id: &str,
        native_id: Option<&str>,
    ) -> Result<bool, String> {
        let recovery = self
            .recovery
            .as_ref()
            .ok_or("Missing Codex recovery registration")?;
        if native_id != Some(recovery.native_id.as_str()) {
            return Err("The recovery registration native identity changed".into());
        }
        let _transition = recovery.lock();
        if self.state() != Self::PENDING {
            return Err(if *recovery.lost.borrow() {
                LEASE_LOST
            } else {
                "Codex recovery registration is no longer pending"
            }
            .into());
        }
        match wardian_core::agent_spawn::mark_codex_recovery_registered(
            operation_id,
            attempt_id,
            &recovery.owner,
            &recovery.agent_id,
            &recovery.native_id,
        )? {
            Some(true) => {
                self.set_committed();
                Ok(true)
            }
            Some(false) => Ok(false),
            None => Err(self.record_loss(recovery)),
        }
    }
}
