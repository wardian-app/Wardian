//! Ephemeral recipient wake signals; the database remains the mailbox authority.

use super::InteractionState;
use tokio::sync::watch;

/// A process-local mailbox snapshot, independent of durable delivery cursors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentMailboxSignal {
    /// Advances for message/reply activity, provider context, and deletion.
    pub revision: u64,
    /// Advances only when the publisher explicitly reports provider context.
    pub provider_revision: u64,
    /// Set on deletion and cleared by explicit session revival.
    pub deleted: bool,
}

impl InteractionState {
    /// Subscribe before reading the durable mailbox to avoid a read/wait race.
    ///
    /// In each receive loop, copy `*receiver.borrow_and_update()` BEFORE the DB
    /// read, releasing that borrow immediately. If the read finds no work and
    /// the snapshot is not deleted, await `receiver.changed()` with no database,
    /// mutation, or lifecycle locks held. A signal during the read remains unseen
    /// and makes that wait return immediately. Re-read the DB after every wake:
    /// watch coalesces activity rather than buffering individual events.
    ///
    /// Signals before subscription are included in the initial snapshot and are
    /// already seen by the new receiver. Inspect `deleted` before waiting, even
    /// on the first iteration. Each subscriber has its own independent seen state.
    pub async fn subscribe_agent_mailbox(
        &self,
        recipient: &str,
    ) -> watch::Receiver<AgentMailboxSignal> {
        self.agent_mailboxes
            .lock()
            .await
            .entry(recipient.to_owned())
            .or_insert_with(|| watch::channel(AgentMailboxSignal::default()).0)
            .subscribe()
    }

    /// Publish a wake AFTER the coordinator's durable transaction commits.
    ///
    /// `provider_context` advances the separate provider revision as well as the
    /// general activity revision. Ordinary message/reply activity leaves the
    /// provider revision unchanged. No DB or lifecycle access is performed here,
    /// and this method never acquires mutation_lock, so callers may already own
    /// that gate. Drop any watch borrows before calling. Deleted mailboxes ignore
    /// late publications until clear_deleted_session revives them.
    pub async fn notify_agent_mailbox(
        &self,
        recipient: &str,
        provider_context: bool,
    ) -> AgentMailboxSignal {
        let mut mailboxes = self.agent_mailboxes.lock().await;
        let sender = mailboxes
            .entry(recipient.to_owned())
            .or_insert_with(|| watch::channel(AgentMailboxSignal::default()).0);
        // send_if_modified retains the snapshot without active subscribers and
        // does not publish another wake for a terminal mailbox.
        sender.send_if_modified(|signal| {
            if signal.deleted {
                return false;
            }
            signal.revision = signal.revision.wrapping_add(1);
            if provider_context {
                signal.provider_revision = signal.provider_revision.wrapping_add(1);
            }
            true
        });
        let snapshot = *sender.borrow();
        snapshot
    }

    /// Called only after durable deletion and live cache invalidation succeed.
    /// Lock order is mutation_lock -> agent_mailboxes; mailbox APIs never reverse it.
    pub(super) async fn mark_agent_mailbox_deleted(&self, recipient: &str) {
        let mut mailboxes = self.agent_mailboxes.lock().await;
        let sender = mailboxes
            .entry(recipient.to_owned())
            .or_insert_with(|| watch::channel(AgentMailboxSignal::default()).0);
        sender.send_if_modified(|signal| {
            if signal.deleted {
                return false;
            }
            signal.deleted = true;
            signal.revision = signal.revision.wrapping_add(1);
            true
        });
    }

    /// Wake existing subscribers on identity reuse without replacing their sender
    /// or resetting either revision. Called under the same mutation gate as deletion.
    pub(super) async fn revive_agent_mailbox(&self, recipient: &str) {
        let mut mailboxes = self.agent_mailboxes.lock().await;
        if let Some(sender) = mailboxes.get_mut(recipient) {
            sender.send_if_modified(|signal| {
                if !signal.deleted {
                    return false;
                }
                signal.deleted = false;
                signal.revision = signal.revision.wrapping_add(1);
                true
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    async fn changed(receiver: &mut watch::Receiver<AgentMailboxSignal>) -> AgentMailboxSignal {
        timeout(Duration::from_secs(1), receiver.changed())
            .await
            .expect("mailbox wake was missed")
            .expect("mailbox sender unexpectedly closed");
        *receiver.borrow_and_update()
    }

    #[tokio::test]
    async fn signal_between_read_and_wait_is_not_missed() {
        let state = InteractionState::default();
        let mut receiver = state.subscribe_agent_mailbox("recipient").await;
        assert_eq!(*receiver.borrow_and_update(), AgentMailboxSignal::default());
        // An empty DB read completes here. Publish before changed() is polled.
        let published = state.notify_agent_mailbox("recipient", false).await;
        assert_eq!(changed(&mut receiver).await, published);
        assert_eq!(published.revision, 1);
        assert_eq!(published.provider_revision, 0);
    }

    #[tokio::test]
    async fn simultaneous_waiters_each_observe_the_same_signal() {
        let state = InteractionState::default();
        let mut first = state.subscribe_agent_mailbox("recipient").await;
        let mut second = state.subscribe_agent_mailbox("recipient").await;
        let mut other = state.subscribe_agent_mailbox("other").await;
        // Poll both waits before publishing, with no timing-dependent sleep.
        let (first_signal, second_signal, published) = tokio::join!(
            changed(&mut first),
            changed(&mut second),
            state.notify_agent_mailbox("recipient", true),
        );
        assert_eq!(first_signal, published);
        assert_eq!(second_signal, published);
        assert!(!other.has_changed().unwrap());
        assert_eq!(*other.borrow_and_update(), AgentMailboxSignal::default());
        let next = state.notify_agent_mailbox("recipient", false).await;
        assert_eq!(changed(&mut first).await, next);
        assert_eq!(changed(&mut second).await, next);
        assert_eq!(next.revision, 2);
        assert_eq!(next.provider_revision, 1);
    }

    #[tokio::test]
    async fn pre_subscription_signals_are_retained_as_seen_baseline() {
        let state = InteractionState::default();
        state.notify_agent_mailbox("recipient", true).await;
        let baseline = state.notify_agent_mailbox("recipient", false).await;
        let mut receiver = state.subscribe_agent_mailbox("recipient").await;
        assert_eq!(*receiver.borrow_and_update(), baseline);
        assert!(!receiver.has_changed().unwrap());
        assert_eq!(baseline.revision, 2);
        assert_eq!(baseline.provider_revision, 1);
        drop(receiver);
        let next = state.notify_agent_mailbox("recipient", true).await;
        let receiver = state.subscribe_agent_mailbox("recipient").await;
        assert_eq!(*receiver.borrow(), next);
        assert!(!receiver.has_changed().unwrap());
        assert_eq!(next.provider_revision, 2);
    }

    #[tokio::test]
    async fn coalesced_activity_preserves_separate_provider_revision() {
        let state = InteractionState::default();
        let mut receiver = state.subscribe_agent_mailbox("recipient").await;
        state.notify_agent_mailbox("recipient", true).await;
        state.notify_agent_mailbox("recipient", false).await;
        state.notify_agent_mailbox("recipient", false).await;
        let signal = changed(&mut receiver).await;
        assert_eq!(signal.revision, 3);
        assert_eq!(signal.provider_revision, 1);
        assert_eq!(state.agent_message_provider_revision("recipient").await, 1);
    }

    #[tokio::test]
    async fn mailbox_publication_does_not_reacquire_mutation_gate() {
        let state = InteractionState::default();
        let mutation = state.mutation_lock.lock().await;
        let mut receiver = timeout(
            Duration::from_secs(1),
            state.subscribe_agent_mailbox("recipient"),
        )
        .await
        .expect("subscription acquired mutation gate");
        let published = timeout(
            Duration::from_secs(1),
            state.notify_agent_mailbox("recipient", false),
        )
        .await
        .expect("publication acquired mutation gate");
        drop(mutation);
        assert_eq!(changed(&mut receiver).await, published);
    }

    #[tokio::test]
    async fn durable_deletion_wakes_waiters_and_retains_terminal_snapshot() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let test_home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&test_home.path().join("state.db")).unwrap();
        let state = InteractionState::default();
        let mut first = state.subscribe_agent_mailbox("recipient").await;
        let mut second = state.subscribe_agent_mailbox("recipient").await;
        state.notify_agent_mailbox("recipient", true).await;
        first.borrow_and_update();
        second.borrow_and_update();
        let (first_signal, second_signal, deletion) = tokio::join!(
            changed(&mut first),
            changed(&mut second),
            state.delete_agent_durable_state("recipient"),
        );
        deletion.unwrap();
        assert_eq!(first_signal, second_signal);
        assert!(first_signal.deleted);
        assert_eq!(first_signal.revision, 2);
        assert_eq!(first_signal.provider_revision, 1);
        let late = state.subscribe_agent_mailbox("recipient").await;
        assert_eq!(*late.borrow(), first_signal);
        assert!(!late.has_changed().unwrap());
        assert_eq!(
            state.notify_agent_mailbox("recipient", true).await,
            first_signal
        );
        state.delete_agent_durable_state("recipient").await.unwrap();
        assert!(!first.has_changed().unwrap());
        // Deletion without any subscribers must also leave a tombstone.
        state
            .delete_agent_durable_state("unobserved")
            .await
            .unwrap();
        let unobserved = state.subscribe_agent_mailbox("unobserved").await;
        assert!(unobserved.borrow().deleted);
        assert!(!unobserved.has_changed().unwrap());
    }

    #[tokio::test]
    async fn revival_wakes_existing_and_resubscribed_waiters_without_resetting_revisions() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let test_home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&test_home.path().join("state.db")).unwrap();
        let state = InteractionState::default();
        let mut existing = state.subscribe_agent_mailbox("recipient").await;
        state.notify_agent_mailbox("recipient", true).await;
        state.delete_agent_durable_state("recipient").await.unwrap();
        let deleted = changed(&mut existing).await;
        assert!(deleted.deleted);
        let mut during_deletion = state.subscribe_agent_mailbox("recipient").await;
        assert_eq!(*during_deletion.borrow(), deleted);
        let (revived, resubscribed, ()) = tokio::join!(
            changed(&mut existing),
            changed(&mut during_deletion),
            state.clear_deleted_session("recipient"),
        );
        assert_eq!(revived, resubscribed);
        assert!(!revived.deleted);
        assert_eq!(revived.revision, deleted.revision + 1);
        assert_eq!(revived.provider_revision, deleted.provider_revision);
        assert!(!state.deleted_sessions.lock().await.contains("recipient"));
        let mut after_revival = state.subscribe_agent_mailbox("recipient").await;
        assert_eq!(*after_revival.borrow(), revived);
        assert!(!after_revival.has_changed().unwrap());
        state.clear_deleted_session("recipient").await;
        assert!(!existing.has_changed().unwrap());
        let published = state.notify_agent_mailbox("recipient", true).await;
        assert_eq!(changed(&mut existing).await, published);
        assert_eq!(changed(&mut during_deletion).await, published);
        assert_eq!(changed(&mut after_revival).await, published);
        assert_eq!(published.provider_revision, revived.provider_revision + 1);
        // If deletion and revival coalesce before a waiter polls, its original
        // sender still reports a change and preserves the current live baseline.
        state.delete_agent_durable_state("recipient").await.unwrap();
        state.clear_deleted_session("recipient").await;
        let coalesced = changed(&mut existing).await;
        assert!(!coalesced.deleted);
        assert_eq!(coalesced.revision, published.revision + 2);
        assert_eq!(coalesced.provider_revision, published.provider_revision);
    }
}
