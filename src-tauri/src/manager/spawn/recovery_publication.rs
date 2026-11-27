use super::{RegistrationPublicationState, PROVIDER_SPAWN_LEASE_DURATION};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use tokio::sync::watch;
use wardian_core::conversation_lease::{
    refresh_codex_recovery_lease_persisted, with_exact_codex_recovery_lease,
    CodexRecoveryLeaseScope, ConversationLeaseOwner,
};

const LEASE_LOST: &str = "Codex recovery lost its exact registration lease";
const AWAITING_ACTIVATION: u8 = 0;
const ACTIVATED: u8 = 1;
const CLEANING_UP: u8 = 2;

pub(super) struct RecoveryPublication {
    owner: ConversationLeaseOwner,
    agent_id: String,
    native_id: String,
    // Serialize watcher loss, cancellation, durable success and activation.
    // The lock covers synchronous effects while the exact lease is pinned.
    transition: Mutex<()>,
    lost: watch::Sender<bool>,
    runtime_outcome: AtomicU8,
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
                runtime_outcome: AtomicU8::default(),
            }),
            ..Default::default()
        }
    }

    pub(crate) fn is_recovery(&self) -> bool {
        self.recovery.is_some()
    }

    pub(crate) fn begin_recovery_cleanup(&self) {
        if let Some(recovery) = &self.recovery {
            let _transition = recovery.lock();
            recovery
                .runtime_outcome
                .store(CLEANING_UP, Ordering::Release);
            self.set_failed();
        } else {
            self.fail();
        }
    }

    pub(super) fn recovery_allows_release(&self) -> bool {
        self.recovery
            .as_ref()
            .is_none_or(|recovery| recovery.runtime_outcome.load(Ordering::Acquire) == ACTIVATED)
    }

    pub(super) fn recovery_cleaning_up(&self) -> bool {
        self.recovery
            .as_ref()
            .is_some_and(|recovery| recovery.runtime_outcome.load(Ordering::Acquire) == CLEANING_UP)
    }

    pub(crate) fn recovery_lease_lost(&self) -> bool {
        self.recovery
            .as_ref()
            .is_some_and(|recovery| *recovery.lost.borrow())
    }

    pub(crate) fn with_recovery_attachment<T>(
        &self,
        agent_id: &str,
        native_id: &str,
        publish: impl FnOnce() -> T,
    ) -> Result<T, String> {
        if self.recovery.as_ref().is_some_and(|recovery| {
            recovery.agent_id != agent_id || recovery.native_id != native_id
        }) {
            return Err("The recovery attachment identity changed".into());
        }
        self.with_recovery_publication(publish)
    }

    pub(crate) fn committed_recovery_native_id(&self, agent_id: &str) -> Result<&str, String> {
        self.recovery
            .as_ref()
            .filter(|recovery| recovery.agent_id == agent_id && self.state() == Self::COMMITTED)
            .map(|recovery| recovery.native_id.as_str())
            .ok_or_else(|| "Exact Codex recovery has not committed registration".into())
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

    /// The durable row is historical evidence. Current input can go ready
    /// only while the same acquisition still excludes competing lifecycles.
    pub(crate) fn with_recovery_activation(
        &self,
        activate: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let recovery = self
            .recovery
            .as_ref()
            .ok_or("Missing Codex recovery registration")?;
        let _transition = recovery.lock();
        if *recovery.lost.borrow() {
            return Err(LEASE_LOST.into());
        }
        if self.state() != Self::COMMITTED
            || recovery.runtime_outcome.load(Ordering::Acquire) != AWAITING_ACTIVATION
        {
            return Err("Codex recovery is no longer awaiting activation".into());
        }
        match recovery.with_lease(|| {
            activate()?;
            recovery.runtime_outcome.store(ACTIVATED, Ordering::Release);
            Ok(())
        }) {
            Ok(Some(result)) => result,
            Ok(None) | Err(_) => Err(self.record_loss(recovery)),
        }
    }

    pub(crate) fn renew_recovery_lease(&self) -> Result<(), String> {
        let recovery = self
            .recovery
            .as_ref()
            .ok_or("Missing Codex recovery registration")?;
        let _transition = recovery.lock();
        if self.state() == Self::FAILED
            || recovery.runtime_outcome.load(Ordering::Acquire) != AWAITING_ACTIVATION
        {
            return Ok(());
        }
        if *recovery.lost.borrow() {
            return Err(LEASE_LOST.into());
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

    /// Keep the watcher through activation even after durable commit.
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
