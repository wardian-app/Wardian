use super::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Notify};
use wardian_core::models::*;

#[derive(Default)]
struct ManualTimer {
    sleepers: Mutex<Vec<(Duration, oneshot::Sender<()>)>>,
    sleeper_registered: Notify,
}

impl ManualTimer {
    async fn fire(&self, duration: Duration) {
        for _ in 0..100 {
            let sender = {
                let mut sleepers = self.sleepers.lock().expect("manual timer lock");
                sleepers.retain(|(_, sender)| !sender.is_closed());
                sleepers
                    .iter()
                    .rposition(|(delay, _)| *delay == duration)
                    .map(|index| sleepers.swap_remove(index).1)
            };
            if let Some(sender) = sender {
                let _ = sender.send(());
                tokio::task::yield_now().await;
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("no sleeper registered for {duration:?}");
    }

    fn has_sleep(&self, duration: Duration) -> bool {
        self.sleepers
            .lock()
            .expect("manual timer lock")
            .iter()
            .any(|(delay, _)| *delay == duration)
    }

    fn live_sleep_count(&self, duration: Duration) -> usize {
        let mut sleepers = self.sleepers.lock().expect("manual timer lock");
        sleepers.retain(|(_, sender)| !sender.is_closed());
        sleepers
            .iter()
            .filter(|(delay, _)| *delay == duration)
            .count()
    }

    async fn wait_for_sleep(&self, duration: Duration) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.has_sleep(duration) {
                    return;
                }
                self.sleeper_registered.notified().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no sleeper registered for {duration:?}"));
    }
}

impl TerminalTimer for ManualTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (tx, rx) = oneshot::channel();
        self.sleepers
            .lock()
            .expect("manual timer lock")
            .push((duration, tx));
        self.sleeper_registered.notify_one();
        Box::pin(async move {
            let _ = rx.await;
        })
    }
}

fn geometry(cols: u16, rows: u16) -> TerminalGeometry {
    TerminalGeometry { rows, cols }
}

type RuntimeFixture = (
    TerminalRuntimeHandles,
    mpsc::Receiver<Vec<u8>>,
    Arc<Mutex<Vec<TerminalGeometry>>>,
);

fn runtime() -> RuntimeFixture {
    let (input_tx, input_rx) = mpsc::channel(256);
    let resizes = Arc::new(Mutex::new(Vec::new()));
    let observed = resizes.clone();
    let runtime = TerminalRuntimeHandles::new(input_tx, move |geometry| {
        observed.lock().expect("resize log").push(geometry);
        Ok(())
    });
    (runtime, input_rx, resizes)
}

async fn start(timer: Arc<ManualTimer>) -> (Arc<TerminalSessionBroker>, u64) {
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer));
    let (runtime, _, _) = runtime();
    let generation = broker
        .start_or_replace_runtime("session-1", runtime, geometry(80, 24))
        .await
        .expect("start runtime");
    (broker, generation)
}

fn registration(
    presentation_id: &str,
    client_kind: TerminalClientKind,
    requested_interaction: TerminalRequestedInteraction,
) -> TerminalPresentationRegistration {
    TerminalPresentationRegistration {
        presentation_id: presentation_id.to_string(),
        session_id: "session-1".to_string(),
        client_kind,
        desired_geometry: Some(geometry(100, 30)),
        visibility: TerminalVisibility::Visible,
        render_state: TerminalRenderState::Mounted,
        requested_interaction,
        observed_lease_epoch: 0,
    }
}

async fn register_desktop(
    broker: &TerminalSessionBroker,
    presentation_id: &str,
) -> TerminalPresentationRegistrationResult {
    broker
        .register_presentation(
            registration(
                presentation_id,
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("register desktop")
}

async fn activate(
    broker: &TerminalSessionBroker,
    presentation_id: &str,
    runtime_generation: u64,
    observed_lease_epoch: u64,
) -> TerminalActivationAckResult {
    let begin = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: presentation_id.to_string(),
            runtime_generation,
            observed_lease_epoch,
        })
        .await
        .expect("begin activation");
    assert_eq!(begin.decision.status, TerminalLeaseDecisionStatus::Accepted);
    broker
        .ack_activation(TerminalActivationAckRequest {
            session_id: "session-1".to_string(),
            presentation_id: presentation_id.to_string(),
            runtime_generation,
            lease_epoch: begin.decision.lease_epoch,
            activation_id: begin.activation_id.expect("activation id"),
        })
        .await
        .expect("ack activation")
}

fn process_output(
    broker: Arc<TerminalSessionBroker>,
    runtime_generation: u64,
    bytes: impl Into<Vec<u8>>,
) -> Result<(), TerminalBrokerError> {
    let bytes = bytes.into();
    std::thread::spawn(move || {
        broker.process_output_blocking("session-1", runtime_generation, bytes)
    })
    .join()
    .expect("output thread")
}

async fn wait_for_external_capacity(broker: &TerminalSessionBroker, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if broker
                .external_command_capacity_for_test("session-1")
                .await
                .expect("command capacity")
                == expected
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("external command capacity never reached {expected}"));
}

async fn wait_for_ack_reservation(broker: &TerminalSessionBroker, activation_id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if broker
                .activation_ack_reserved_for_test("session-1", activation_id)
                .await
                .expect("ack reservation diagnostic")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("ack was never reserved before deadline");
}

async fn wait_for_ack_reservation_count(
    broker: &TerminalSessionBroker,
    activation_id: &str,
    expected: usize,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if broker
                .activation_ack_reservation_count_for_test("session-1", activation_id)
                .await
                .expect("ack reservation count diagnostic")
                == expected
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("ack reservation count never reached {expected}"));
}

async fn wait_for_activation_deadline_elapsed(broker: &TerminalSessionBroker, activation_id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if broker
                .activation_deadline_has_elapsed_for_test("session-1", activation_id)
                .await
                .expect("activation deadline diagnostic")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("activation deadline never elapsed");
}

async fn wait_for_live_sleep_count(timer: &ManualTimer, duration: Duration, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if timer.live_sleep_count(duration) == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("live sleeper count never reached {expected}"));
}

#[test]
fn actor_control_path_does_not_use_an_unbounded_channel() {
    assert!(!include_str!("actor.rs").contains("unbounded_channel"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_is_passive_and_capability_cannot_escalate() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, _) = start(timer).await;

    let desktop = register_desktop(&broker, "desktop-1").await;
    assert_eq!(
        desktop.presentation.interaction_capability,
        TerminalInteractionCapability::Interactive
    );
    assert_eq!(desktop.broker_state.owner_presentation_id, None);
    assert_eq!(desktop.broker_state.lease_epoch, 0);

    let downgraded = broker
        .register_presentation(
            registration(
                "desktop-read-only",
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::ReadOnly,
            ),
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("read-only desktop");
    assert_eq!(
        downgraded.presentation.interaction_capability,
        TerminalInteractionCapability::ReadOnly
    );

    let remote = broker
        .register_presentation(
            registration(
                "attachment-1",
                TerminalClientKind::Remote,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::authenticated_remote("attachment-1", false),
        )
        .await
        .expect("policy-read-only remote");
    assert_eq!(
        remote.presentation.interaction_capability,
        TerminalInteractionCapability::ReadOnly
    );

    let mismatch = broker
        .register_presentation(
            registration(
                "pretend-desktop",
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::authenticated_remote("pretend-desktop", true),
        )
        .await;
    assert!(matches!(
        mismatch,
        Err(TerminalBrokerError::InvalidIdentity)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identifiers_over_utf8_byte_limit_are_rejected_without_truncation() {
    let timer = Arc::new(ManualTimer::default());
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer));
    let oversized = "é".repeat(257);
    assert_eq!(oversized.len(), 514);

    let (oversized_runtime, _, _) = runtime();
    let session_result = broker
        .start_or_replace_runtime(&oversized, oversized_runtime, geometry(80, 24))
        .await;
    assert!(matches!(
        session_result,
        Err(TerminalBrokerError::InvalidRequest("session_id"))
    ));

    let (valid_runtime, _, _) = runtime();
    let generation = broker
        .start_or_replace_runtime("session-1", valid_runtime, geometry(80, 24))
        .await
        .expect("valid runtime");
    let presentation_result = broker
        .register_presentation(
            registration(
                &oversized,
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::trusted_desktop(),
        )
        .await;
    assert!(matches!(
        presentation_result,
        Err(TerminalBrokerError::InvalidRequest("presentation_id"))
    ));
    let consumer_result = broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: oversized,
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await;
    assert!(matches!(
        consumer_result,
        Err(TerminalBrokerError::InvalidRequest("consumer_id"))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_and_remote_presentation_limits_are_independent() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, _) = start(timer).await;

    for index in 0..64 {
        register_desktop(&broker, &format!("desktop-{index}")).await;
    }
    let desktop_overflow = broker
        .register_presentation(
            registration(
                "desktop-overflow",
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::trusted_desktop(),
        )
        .await;
    assert!(matches!(
        desktop_overflow,
        Err(TerminalBrokerError::PresentationLimit {
            client_kind: TerminalClientKind::Desktop,
            limit: 64
        })
    ));

    for index in 0..3 {
        let id = format!("remote-{index}");
        broker
            .register_presentation(
                registration(
                    &id,
                    TerminalClientKind::Remote,
                    TerminalRequestedInteraction::Interactive,
                ),
                TerminalClientIdentity::authenticated_remote(id, true),
            )
            .await
            .expect("remote within independent limit");
    }
    let remote_overflow = broker
        .register_presentation(
            registration(
                "remote-overflow",
                TerminalClientKind::Remote,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::authenticated_remote("remote-overflow", true),
        )
        .await;
    assert!(matches!(
        remote_overflow,
        Err(TerminalBrokerError::PresentationLimit {
            client_kind: TerminalClientKind::Remote,
            limit: 3
        })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activation_is_two_phase_idempotent_and_superseding() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "one").await;
    register_desktop(&broker, "two").await;

    let request = TerminalActivationBeginRequest {
        session_id: "session-1".to_string(),
        presentation_id: "one".to_string(),
        runtime_generation: generation,
        observed_lease_epoch: 0,
    };
    let first = broker
        .begin_activation(request.clone())
        .await
        .expect("first begin");
    let duplicate = broker
        .begin_activation(request)
        .await
        .expect("duplicate begin");
    assert_eq!(first.activation_id, duplicate.activation_id);
    assert_eq!(first.decision.lease_epoch, duplicate.decision.lease_epoch);

    let newer = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "two".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: first.decision.lease_epoch,
        })
        .await
        .expect("superseding begin");
    assert_ne!(first.activation_id, newer.activation_id);

    let stale = broker
        .ack_activation(TerminalActivationAckRequest {
            session_id: "session-1".to_string(),
            presentation_id: "one".to_string(),
            runtime_generation: generation,
            lease_epoch: first.decision.lease_epoch,
            activation_id: first.activation_id.expect("old activation"),
        })
        .await
        .expect("structured stale ack");
    assert_eq!(stale.decision.status, TerminalLeaseDecisionStatus::Rejected);
    assert_eq!(
        stale.decision.reason,
        Some(TerminalLeaseRejectionReason::StaleActivation)
    );

    let ack_request = TerminalActivationAckRequest {
        session_id: "session-1".to_string(),
        presentation_id: "two".to_string(),
        runtime_generation: generation,
        lease_epoch: newer.decision.lease_epoch,
        activation_id: newer.activation_id.expect("new activation"),
    };
    let committed = broker
        .ack_activation(ack_request.clone())
        .await
        .expect("commit");
    let duplicate_ack = broker
        .ack_activation(ack_request)
        .await
        .expect("duplicate ack");
    assert_eq!(committed, duplicate_ack);
    assert_eq!(
        committed.broker_state.owner_presentation_id.as_deref(),
        Some("two")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_activation_freezes_input_and_timeout_rolls_back() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "one").await;
    register_desktop(&broker, "two").await;
    let owner = activate(&broker, "one", generation, 0).await;

    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "two".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: owner.broker_state.lease_epoch,
        })
        .await
        .expect("begin transfer");
    for presentation_id in ["one", "two"] {
        let decision = broker
            .send_input(TerminalInputRequest {
                lease: TerminalLeaseIdentity {
                    session_id: "session-1".to_string(),
                    presentation_id: presentation_id.to_string(),
                    runtime_generation: generation,
                    lease_epoch: pending.decision.lease_epoch,
                },
                bytes: b"blocked".to_vec(),
            })
            .await
            .expect("structured pending decision");
        assert_eq!(
            decision.reason,
            Some(TerminalLeaseRejectionReason::PendingActivation)
        );
    }

    timer.fire(Duration::from_secs(5)).await;
    let mut state = broker
        .broker_state("session-1")
        .await
        .expect("state after timer release");
    for _ in 0..100 {
        if state.pending_activation.is_none() {
            break;
        }
        tokio::task::yield_now().await;
        state = broker
            .broker_state("session-1")
            .await
            .expect("state while timeout settles");
    }
    assert_eq!(state.owner_presentation_id.as_deref(), Some("one"));
    assert_eq!(state.lease_epoch, pending.decision.lease_epoch);
    assert!(state.pending_activation.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_during_pending_restores_previous_owner() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "one").await;
    register_desktop(&broker, "two").await;
    let owner = activate(&broker, "one", generation, 0).await;
    broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "two".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: owner.broker_state.lease_epoch,
        })
        .await
        .expect("pending transfer");

    broker
        .unregister_presentation("session-1", "two", generation)
        .await
        .expect("disconnect proposed owner");
    let state = broker
        .broker_state("session-1")
        .await
        .expect("state after disconnect");
    assert_eq!(state.owner_presentation_id.as_deref(), Some("one"));
    assert!(state.pending_activation.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_loss_promotes_latest_eligible_server_sequence() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "older").await;
    register_desktop(&broker, "newer").await;
    register_desktop(&broker, "owner").await;
    let active = activate(&broker, "owner", generation, 0).await;

    broker
        .unregister_presentation("session-1", "owner", generation)
        .await
        .expect("owner disconnect");
    let state = broker
        .broker_state("session-1")
        .await
        .expect("promoted state");
    assert_eq!(state.owner_presentation_id.as_deref(), Some("newer"));
    assert!(state.lease_epoch > active.broker_state.lease_epoch);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_read_only_and_suspended_presentations_are_not_promoted() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    for id in ["eligible", "hidden", "suspended"] {
        register_desktop(&broker, id).await;
    }
    broker
        .register_presentation(
            registration(
                "read-only",
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::ReadOnly,
            ),
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("read-only mirror");
    register_desktop(&broker, "owner").await;

    for (id, visibility, render_state) in [
        (
            "hidden",
            TerminalVisibility::Hidden,
            TerminalRenderState::Mounted,
        ),
        (
            "suspended",
            TerminalVisibility::Visible,
            TerminalRenderState::Suspended,
        ),
    ] {
        broker
            .update_presentation(
                TerminalPresentationUpdateRequest {
                    presentation_id: id.to_string(),
                    session_id: "session-1".to_string(),
                    runtime_generation: generation,
                    desired_geometry: Some(geometry(90, 20)),
                    visibility,
                    render_state,
                    requested_interaction: TerminalRequestedInteraction::Interactive,
                    observed_lease_epoch: 0,
                },
                TerminalClientIdentity::trusted_desktop(),
            )
            .await
            .expect("update eligibility");
    }
    activate(&broker, "owner", generation, 0).await;
    broker
        .unregister_presentation("session-1", "owner", generation)
        .await
        .expect("owner loss");
    let state = broker
        .broker_state("session-1")
        .await
        .expect("promotion state");
    assert_eq!(state.owner_presentation_id.as_deref(), Some("eligible"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspended_owner_must_apply_activation_snapshot_before_input_resumes() {
    let timer = Arc::new(ManualTimer::default());
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer));
    let (runtime, mut input_rx, _) = runtime();
    let generation = broker
        .start_or_replace_runtime("session-1", runtime, geometry(80, 24))
        .await
        .expect("start runtime");
    register_desktop(&broker, "owner").await;
    let active = activate(&broker, "owner", generation, 0).await;

    for render_state in [TerminalRenderState::Suspended, TerminalRenderState::Mounted] {
        broker
            .update_presentation(
                TerminalPresentationUpdateRequest {
                    presentation_id: "owner".to_string(),
                    session_id: "session-1".to_string(),
                    runtime_generation: generation,
                    desired_geometry: Some(geometry(100, 30)),
                    visibility: TerminalVisibility::Visible,
                    render_state,
                    requested_interaction: TerminalRequestedInteraction::Interactive,
                    observed_lease_epoch: active.broker_state.lease_epoch,
                },
                TerminalClientIdentity::trusted_desktop(),
            )
            .await
            .expect("update renderer state");
    }
    broker
        .register_presentation(
            registration(
                "owner",
                TerminalClientKind::Desktop,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("idempotent re-registration must not bypass resync");
    let before_resync = broker
        .broker_state("session-1")
        .await
        .expect("state before passive resync");
    let blocked = broker
        .send_input(TerminalInputRequest {
            lease: TerminalLeaseIdentity {
                session_id: "session-1".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch: active.broker_state.lease_epoch,
            },
            bytes: b"before resync".to_vec(),
        })
        .await
        .expect("structured pre-resync decision");
    assert_eq!(
        blocked.reason,
        Some(TerminalLeaseRejectionReason::PresentationIneligible)
    );
    let begin = broker
        .begin_owner_resync(TerminalOwnerResyncBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "owner".to_string(),
            runtime_generation: generation,
            lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin passive owner resync");
    assert_eq!(begin.decision.status, TerminalLeaseDecisionStatus::Accepted);
    assert_eq!(begin.sequence_barrier, before_resync.stream_sequence);
    let during_resync = broker
        .broker_state("session-1")
        .await
        .expect("state during passive resync");
    assert_eq!(during_resync, before_resync);

    let ack = broker
        .ack_owner_resync(TerminalOwnerResyncAckRequest {
            session_id: "session-1".to_string(),
            presentation_id: "owner".to_string(),
            runtime_generation: generation,
            lease_epoch: active.broker_state.lease_epoch,
            resync_id: begin.resync_id.expect("broker resync id"),
        })
        .await
        .expect("ack passive owner resync");
    assert_eq!(ack.decision.status, TerminalLeaseDecisionStatus::Accepted);
    assert_eq!(ack.broker_state, before_resync);

    let accepted = broker
        .send_input(TerminalInputRequest {
            lease: TerminalLeaseIdentity {
                session_id: "session-1".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch: active.broker_state.lease_epoch,
            },
            bytes: b"after resync".to_vec(),
        })
        .await
        .expect("input after resync");
    assert_eq!(accepted.status, TerminalLeaseDecisionStatus::Accepted);
    assert_eq!(
        input_rx.recv().await.as_deref(),
        Some(b"after resync".as_slice())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_takeover_invalidates_stale_owner_resync_ack() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    broker
        .update_presentation(
            TerminalPresentationUpdateRequest {
                presentation_id: "owner".to_string(),
                session_id: "session-1".to_string(),
                runtime_generation: generation,
                desired_geometry: Some(geometry(100, 30)),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Suspended,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: active.broker_state.lease_epoch,
            },
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("suspend owner");
    broker
        .update_presentation(
            TerminalPresentationUpdateRequest {
                presentation_id: "owner".to_string(),
                session_id: "session-1".to_string(),
                runtime_generation: generation,
                desired_geometry: Some(geometry(100, 30)),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Mounted,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: active.broker_state.lease_epoch,
            },
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("remount owner");
    let resync = broker
        .begin_owner_resync(TerminalOwnerResyncBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "owner".to_string(),
            runtime_generation: generation,
            lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin owner resync");
    let takeover = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin explicit takeover");
    let stale = broker
        .ack_owner_resync(TerminalOwnerResyncAckRequest {
            session_id: "session-1".to_string(),
            presentation_id: "owner".to_string(),
            runtime_generation: generation,
            lease_epoch: active.broker_state.lease_epoch,
            resync_id: resync.resync_id.expect("resync id"),
        })
        .await
        .expect("structured stale resync ack");
    assert_eq!(
        stale.decision.reason,
        Some(TerminalLeaseRejectionReason::StaleOwnerResync)
    );
    assert_eq!(
        stale
            .broker_state
            .pending_activation
            .as_ref()
            .map(|pending| pending.activation_id.as_str()),
        takeover.activation_id.as_deref()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_presentations_do_not_destroy_runtime_state() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "only").await;
    broker
        .unregister_presentation("session-1", "only", generation)
        .await
        .expect("remove only presentation");

    process_output(broker.clone(), generation, b"still alive".to_vec())
        .expect("output without presentations");
    let snapshot = broker
        .snapshot("session-1")
        .await
        .expect("retained runtime snapshot");
    assert!(snapshot.visible_grid.contains("still alive"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn never_activated_remote_is_not_fallback_promoted_when_owner_detaches() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "desktop-owner").await;
    let active = activate(&broker, "desktop-owner", generation, 0).await;
    broker
        .register_presentation(
            registration(
                "passive-remote",
                TerminalClientKind::Remote,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::authenticated_remote("passive-remote", true),
        )
        .await
        .expect("register passive remote");

    let state = broker
        .unregister_presentation("session-1", "desktop-owner", generation)
        .await
        .expect("detach owner");
    assert_eq!(
        active.broker_state.owner_presentation_id.as_deref(),
        Some("desktop-owner")
    );
    assert_eq!(state.owner_presentation_id, None);

    let begin = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "passive-remote".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: state.lease_epoch,
        })
        .await
        .expect("passive remote may still explicitly activate");
    assert_eq!(begin.decision.status, TerminalLeaseDecisionStatus::Accepted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_timeout_does_not_wedge_subsequent_broker_calls() {
    let timer = Arc::new(ManualTimer::default());
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer));
    let (input_tx, _input_rx) = mpsc::channel(1);
    input_tx
        .send(b"fills native input channel".to_vec())
        .await
        .expect("prefill input channel");
    let runtime = TerminalRuntimeHandles::new(input_tx, |_| Ok(()));
    let generation = broker
        .start_or_replace_runtime("session-1", runtime, geometry(80, 24))
        .await
        .expect("start runtime");
    register_desktop(&broker, "owner").await;
    let active = activate(&broker, "owner", generation, 0).await;

    let error = tokio::time::timeout(
        Duration::from_secs(3),
        broker.send_input(TerminalInputRequest {
            lease: TerminalLeaseIdentity {
                session_id: "session-1".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch: active.broker_state.lease_epoch,
            },
            bytes: b"cannot enter full native channel".to_vec(),
        }),
    )
    .await
    .expect("actor-owned two second timeout")
    .expect_err("full native channel rejects timed out input");
    assert_eq!(
        error,
        TerminalBrokerError::RuntimeIo("input_timeout".to_string())
    );

    let state = tokio::time::timeout(Duration::from_millis(100), broker.broker_state("session-1"))
        .await
        .expect("actor remains responsive after input timeout")
        .expect("broker state");
    assert_eq!(state.runtime_generation, generation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_actor_backpressure_preserves_every_output_block() {
    let timer = Arc::new(ManualTimer::default());
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer));
    let (input_tx, _) = mpsc::channel(1);
    let (resize_started_tx, resize_started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let runtime = TerminalRuntimeHandles::new(input_tx, move |_| {
        let _ = resize_started_tx.send(());
        let _ = release_rx.lock().expect("release lock").recv();
        Ok(())
    });
    let generation = broker
        .start_or_replace_runtime("session-1", runtime, geometry(80, 24))
        .await
        .expect("start runtime");
    let mut owner_registration = registration(
        "owner",
        TerminalClientKind::Desktop,
        TerminalRequestedInteraction::Interactive,
    );
    owner_registration.desired_geometry = Some(geometry(80, 24));
    broker
        .register_presentation(
            owner_registration,
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("register owner without initial resize");
    let active = activate(&broker, "owner", generation, 0).await;

    let resize_broker = broker.clone();
    let resize = tokio::spawn(async move {
        resize_broker
            .resize(TerminalGeometryRequest {
                lease: TerminalLeaseIdentity {
                    session_id: "session-1".to_string(),
                    presentation_id: "owner".to_string(),
                    runtime_generation: generation,
                    lease_epoch: active.broker_state.lease_epoch,
                },
                geometry_sequence: 1,
                geometry: geometry(81, 25),
            })
            .await
    });
    tokio::task::spawn_blocking(move || resize_started_rx.recv().expect("resize started"))
        .await
        .expect("wait for resize");

    let mut output_threads = Vec::new();
    for index in 0..300_u16 {
        let broker = broker.clone();
        output_threads.push(std::thread::spawn(move || {
            broker.process_output_blocking("session-1", generation, index.to_le_bytes().to_vec())
        }));
    }
    tokio::task::yield_now().await;
    release_tx.send(()).expect("release resize");
    resize.await.expect("resize task").expect("resize result");
    for thread in output_threads {
        thread
            .join()
            .expect("output thread")
            .expect("output accepted");
    }

    let subscription = broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");
    let batch = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: subscription
                .initial_snapshot
                .sequence_barrier
                .saturating_sub(301),
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("first batch");
    let second = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: batch.next_sequence,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("second batch");
    let output_count = batch
        .events
        .iter()
        .chain(second.events.iter())
        .filter(|event| matches!(event.event, TerminalBrokerEventKind::Output { .. }))
        .count();
    assert_eq!(output_count, 300);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_batches_acknowledges_and_unsubscribes_stateless_cursors() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    let subscribed = broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");
    assert_eq!(subscribed.initial_snapshot.sequence_barrier, 0);
    assert_eq!(subscribed.broker_state.stream_sequence, 0);

    for bytes in [b"one".as_slice(), b"two", b"three"] {
        process_output(broker.clone(), generation, bytes.to_vec()).expect("output");
    }
    let first = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 0,
            max_events: 2,
            max_bytes: 7,
        })
        .await
        .expect("bounded read");
    assert_eq!(first.status, TerminalEventBatchStatus::Events);
    assert_eq!(first.events.len(), 2);
    assert_eq!(first.next_sequence, 2);

    let ack = broker
        .ack_events(TerminalEventAckRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            applied_sequence: first.next_sequence,
        })
        .await
        .expect("ack");
    assert_eq!(ack.accepted_sequence, 2);
    broker
        .unsubscribe(TerminalEventUnsubscribeRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
        })
        .await
        .expect("unsubscribe");
    let after_unsubscribe = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 2,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await;
    assert!(matches!(
        after_unsubscribe,
        Err(TerminalBrokerError::ConsumerNotFound)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_output_is_chunked_so_strict_batch_limits_always_progress() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");
    process_output(broker.clone(), generation, vec![b'z'; 300 * 1_024]).expect("oversized output");

    let first = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 0,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("first bounded chunk");
    assert_eq!(first.next_sequence, 1);
    assert_eq!(first.events.len(), 1);
    let first_bytes = match &first.events[0].event {
        TerminalBrokerEventKind::Output { bytes } => bytes.len(),
        event => panic!("expected output, got {event:?}"),
    };
    assert_eq!(first_bytes, 262_144);

    let second = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: first.next_sequence,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("second bounded chunk");
    assert_eq!(second.next_sequence, 2);
    assert_eq!(second.events.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_byte_batch_limit_returns_snapshot_recovery_instead_of_stalling() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");
    process_output(broker.clone(), generation, b"cannot fit".to_vec()).expect("output");

    let recovery = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 0,
            max_events: 256,
            max_bytes: 1,
        })
        .await
        .expect("bounded recovery");
    assert_eq!(recovery.status, TerminalEventBatchStatus::Gap);
    assert_eq!(recovery.next_sequence, recovery.latest_sequence);
    assert!(recovery.next_sequence > 0);
    assert_eq!(
        recovery
            .recovery_snapshot
            .as_ref()
            .expect("recovery snapshot")
            .sequence_barrier,
        recovery.next_sequence
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn escape_heavy_snapshot_serialization_never_exceeds_two_mibibytes() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    let line = "\\\"\u{0007}".repeat(500);
    let output = (0..1_200)
        .map(|_| format!("{line}\r\n"))
        .collect::<String>();
    process_output(broker.clone(), generation, output.into_bytes()).expect("large output");

    let snapshot = broker
        .snapshot("session-1")
        .await
        .expect("bounded snapshot");
    assert!(serde_json::to_vec(&snapshot).expect("serialize").len() <= 2 * 1_024 * 1_024);
    assert!(snapshot.scrollback.len() <= 1_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumers_share_one_ring_but_keep_independent_acknowledgements() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    for (consumer_id, client_kind) in [
        ("desktop", TerminalClientKind::Desktop),
        ("remote", TerminalClientKind::Remote),
    ] {
        broker
            .subscribe(TerminalEventSubscriptionRequest {
                session_id: "session-1".to_string(),
                consumer_id: consumer_id.to_string(),
                client_kind,
                runtime_generation: generation,
            })
            .await
            .expect("subscribe consumer");
    }
    process_output(broker.clone(), generation, b"shared".to_vec()).expect("output");
    for consumer_id in ["desktop", "remote"] {
        let batch = broker
            .read_events(TerminalEventReadRequest {
                session_id: "session-1".to_string(),
                consumer_id: consumer_id.to_string(),
                runtime_generation: generation,
                after_sequence: 0,
                max_events: 256,
                max_bytes: 262_144,
            })
            .await
            .expect("shared read");
        assert_eq!(batch.events.len(), 1);
    }
    broker
        .ack_events(TerminalEventAckRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            applied_sequence: 1,
        })
        .await
        .expect("desktop ack");
    let diagnostics = broker
        .consumer_acknowledgements("session-1")
        .await
        .expect("ack diagnostics");
    assert_eq!(diagnostics.get("desktop"), Some(&1));
    assert_eq!(diagnostics.get("remote"), Some(&0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wakeups_are_coalesced_to_one_per_sixteen_milliseconds() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    let mut wakeups = broker.subscribe_wakeups();

    for bytes in [b"a".as_slice(), b"b", b"c"] {
        process_output(broker.clone(), generation, bytes.to_vec()).expect("output");
    }
    assert!(wakeups.try_recv().is_err());
    timer.fire(Duration::from_millis(16)).await;
    let wake = wakeups.recv().await.expect("coalesced wake");
    assert_eq!(wake.latest_sequence, 3);
    assert!(wakeups.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_limits_produce_gap_snapshot_recovery() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");
    for _ in 0..4_100 {
        process_output(broker.clone(), generation, vec![b'x']).expect("output");
    }
    let gap = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 0,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("gap read");
    assert_eq!(gap.status, TerminalEventBatchStatus::Gap);
    assert_eq!(gap.available_from_sequence, 5);
    assert_eq!(gap.next_sequence, 4_100);
    assert_eq!(
        gap.recovery_snapshot
            .as_ref()
            .expect("recovery snapshot")
            .sequence_barrier,
        4_100
    );

    let (byte_broker, byte_generation) = start(Arc::new(ManualTimer::default())).await;
    byte_broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: byte_generation,
        })
        .await
        .expect("byte subscribe");
    for _ in 0..1_025 {
        process_output(byte_broker.clone(), byte_generation, vec![b'y'; 1_024])
            .expect("byte output");
    }
    let byte_gap = byte_broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: byte_generation,
            after_sequence: 0,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("byte gap");
    assert_eq!(byte_gap.status, TerminalEventBatchStatus::Gap);
    assert_eq!(byte_gap.available_from_sequence, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_change_and_stale_output_are_structured() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, first_generation) = start(timer).await;
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: first_generation,
        })
        .await
        .expect("subscribe first generation");
    process_output(broker.clone(), first_generation, b"old".to_vec()).expect("old output");

    let (replacement, _, _) = runtime();
    let second_generation = broker
        .start_or_replace_runtime("session-1", replacement, geometry(120, 40))
        .await
        .expect("replace runtime");
    assert_eq!(second_generation, first_generation + 1);
    let stale_output = process_output(broker.clone(), first_generation, b"stale".to_vec());
    assert!(matches!(
        stale_output,
        Err(TerminalBrokerError::StaleRuntimeGeneration { .. })
    ));

    let changed = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: first_generation,
            after_sequence: 1,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("generation response");
    assert_eq!(changed.status, TerminalEventBatchStatus::GenerationChanged);
    assert_eq!(changed.runtime_generation, second_generation);
    assert_eq!(changed.next_sequence, 0);
    assert_eq!(
        changed
            .recovery_snapshot
            .expect("generation snapshot")
            .geometry,
        geometry(120, 40)
    );
    let state = broker
        .broker_state("session-1")
        .await
        .expect("replacement state");
    assert_eq!(state.stream_sequence, 0);
    assert_eq!(state.owner_presentation_id, None);
    assert!(state.lease_epoch > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_utf8_and_concurrent_geometry_output_stay_ordered() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "owner").await;
    let active = activate(&broker, "owner", generation, 0).await;
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");

    let glyph = "🙂".as_bytes();
    process_output(broker.clone(), generation, glyph[..2].to_vec()).expect("utf8 head");
    process_output(broker.clone(), generation, glyph[2..].to_vec()).expect("utf8 tail");

    let output_broker = broker.clone();
    let output = std::thread::spawn(move || {
        output_broker.process_output_blocking("session-1", generation, b"ordered".to_vec())
    });
    let resize = broker
        .resize(TerminalGeometryRequest {
            lease: TerminalLeaseIdentity {
                session_id: "session-1".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch: active.broker_state.lease_epoch,
            },
            geometry_sequence: 1,
            geometry: geometry(140, 50),
        })
        .await
        .expect("ordered resize");
    output
        .join()
        .expect("output thread")
        .expect("ordered output");

    assert_eq!(
        resize.decision.status,
        TerminalLeaseDecisionStatus::Accepted
    );
    let snapshot = broker.snapshot("session-1").await.expect("final snapshot");
    assert_eq!(snapshot.geometry, geometry(140, 50));
    assert!(snapshot.visible_grid.contains('🙂'));
    assert!(snapshot.visible_grid.contains("ordered"));

    let batch = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 0,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("ordered stream");
    for pair in batch.events.windows(2) {
        assert_eq!(pair[1].sequence, pair[0].sequence + 1);
    }
    assert!(batch.events.iter().any(|event| matches!(
        event.event,
        TerminalBrokerEventKind::Geometry {
            geometry: TerminalGeometry {
                cols: 140,
                rows: 50
            },
            geometry_sequence: 1
        }
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_geometry_sequence_is_nonfatal_and_limits_depend_on_client_kind() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "desktop").await;
    let active = activate(&broker, "desktop", generation, 0).await;
    let lease = TerminalLeaseIdentity {
        session_id: "session-1".to_string(),
        presentation_id: "desktop".to_string(),
        runtime_generation: generation,
        lease_epoch: active.broker_state.lease_epoch,
    };
    let accepted = broker
        .resize(TerminalGeometryRequest {
            lease: lease.clone(),
            geometry_sequence: 7,
            geometry: geometry(900, 900),
        })
        .await
        .expect("desktop resize");
    assert_eq!(accepted.geometry, geometry(500, 200));
    let stale = broker
        .resize(TerminalGeometryRequest {
            lease,
            geometry_sequence: 7,
            geometry: geometry(40, 10),
        })
        .await
        .expect("structured stale geometry");
    assert_eq!(
        stale.decision.reason,
        Some(TerminalLeaseRejectionReason::StaleGeometrySequence)
    );
    assert_eq!(stale.geometry, geometry(500, 200));

    broker
        .register_presentation(
            registration(
                "remote-owner",
                TerminalClientKind::Remote,
                TerminalRequestedInteraction::Interactive,
            ),
            TerminalClientIdentity::authenticated_remote("remote-owner", true),
        )
        .await
        .expect("register remote owner");
    let remote = activate(
        &broker,
        "remote-owner",
        generation,
        active.broker_state.lease_epoch,
    )
    .await;
    let remote_resize = broker
        .resize(TerminalGeometryRequest {
            lease: TerminalLeaseIdentity {
                session_id: "session-1".to_string(),
                presentation_id: "remote-owner".to_string(),
                runtime_generation: generation,
                lease_epoch: remote.broker_state.lease_epoch,
            },
            geometry_sequence: 1,
            geometry: geometry(900, 900),
        })
        .await
        .expect("remote resize");
    assert_eq!(remote_resize.geometry, geometry(240, 80));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_geometry_ack_does_not_return_a_replay_snapshot() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer).await;
    register_desktop(&broker, "owner").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let current = active.broker_state.geometry;

    let result = broker
        .resize(TerminalGeometryRequest {
            lease: TerminalLeaseIdentity {
                session_id: "session-1".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch: active.broker_state.lease_epoch,
            },
            geometry_sequence: 1,
            geometry: current,
        })
        .await
        .expect("unchanged resize acknowledgement");

    assert_eq!(
        result.decision.status,
        TerminalLeaseDecisionStatus::Accepted
    );
    assert_eq!(result.geometry, current);
    assert!(result.snapshot.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn termination_closes_consumers_and_uses_two_second_shutdown_budget() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: generation,
        })
        .await
        .expect("subscribe");

    broker
        .terminate_runtime("session-1", generation)
        .await
        .expect("terminate");
    assert!(timer.has_sleep(Duration::from_secs(2)));
    let terminated = broker
        .read_events(TerminalEventReadRequest {
            session_id: "session-1".to_string(),
            consumer_id: "desktop".to_string(),
            runtime_generation: generation,
            after_sequence: 0,
            max_events: 256,
            max_bytes: 262_144,
        })
        .await
        .expect("terminated batch");
    assert_eq!(terminated.status, TerminalEventBatchStatus::Terminated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_timeout_aborts_actor_and_cancels_pending_calls() {
    let timer = Arc::new(ManualTimer::default());
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer.clone()));
    let (input_tx, _input_rx) = mpsc::channel(1);
    let runtime = TerminalRuntimeHandles::new(input_tx, |_| Ok(()));
    let generation = broker
        .start_or_replace_runtime("session-1", runtime, geometry(80, 24))
        .await
        .expect("start runtime");
    register_desktop(&broker, "owner").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let lease = TerminalLeaseIdentity {
        session_id: "session-1".to_string(),
        presentation_id: "owner".to_string(),
        runtime_generation: generation,
        lease_epoch: active.broker_state.lease_epoch,
    };
    broker
        .send_input(TerminalInputRequest {
            lease: lease.clone(),
            bytes: b"fills input channel".to_vec(),
        })
        .await
        .expect("first input");

    let blocked_broker = broker.clone();
    let blocked_input = tokio::spawn(async move {
        blocked_broker
            .send_input(TerminalInputRequest {
                lease,
                bytes: b"blocks actor".to_vec(),
            })
            .await
    });
    tokio::task::yield_now().await;
    let state_broker = broker.clone();
    let pending_state = tokio::spawn(async move { state_broker.broker_state("session-1").await });
    let terminate_broker = broker.clone();
    let terminate = tokio::spawn(async move {
        terminate_broker
            .terminate_runtime("session-1", generation)
            .await
    });
    timer.wait_for_sleep(Duration::from_secs(2)).await;
    timer.fire(Duration::from_secs(2)).await;

    terminate
        .await
        .expect("terminate task")
        .expect("timed abort");
    assert!(matches!(
        blocked_input.await.expect("blocked input task"),
        Err(TerminalBrokerError::RuntimeTerminated)
    ));
    assert!(matches!(
        pending_state.await.expect("pending state task"),
        Err(TerminalBrokerError::RuntimeTerminated)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_deadline_includes_waiting_to_enqueue_into_full_actor_queue() {
    let timer = Arc::new(ManualTimer::default());
    let broker = Arc::new(TerminalSessionBroker::with_timer(timer.clone()));
    let (input_tx, _) = mpsc::channel(1);
    let (resize_started_tx, resize_started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let runtime = TerminalRuntimeHandles::new(input_tx, move |_| {
        let _ = resize_started_tx.send(());
        let _ = release_rx.lock().expect("release lock").recv();
        Ok(())
    });
    let generation = broker
        .start_or_replace_runtime("session-1", runtime, geometry(80, 24))
        .await
        .expect("start runtime");
    let mut owner_registration = registration(
        "owner",
        TerminalClientKind::Desktop,
        TerminalRequestedInteraction::Interactive,
    );
    owner_registration.desired_geometry = Some(geometry(80, 24));
    broker
        .register_presentation(
            owner_registration,
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("register owner");
    let active = activate(&broker, "owner", generation, 0).await;
    let resize_broker = broker.clone();
    let resize = tokio::spawn(async move {
        resize_broker
            .resize(TerminalGeometryRequest {
                lease: TerminalLeaseIdentity {
                    session_id: "session-1".to_string(),
                    presentation_id: "owner".to_string(),
                    runtime_generation: generation,
                    lease_epoch: active.broker_state.lease_epoch,
                },
                geometry_sequence: 1,
                geometry: geometry(81, 25),
            })
            .await
    });
    tokio::task::spawn_blocking(move || resize_started_rx.recv().expect("resize started"))
        .await
        .expect("resize start waiter");

    let mut output_threads = Vec::new();
    for index in 0..TERMINAL_SESSION_ACTOR_CAPACITY {
        let output_broker = broker.clone();
        output_threads.push(std::thread::spawn(move || {
            output_broker.process_output_blocking(
                "session-1",
                generation,
                vec![(index % 251) as u8],
            )
        }));
    }
    wait_for_external_capacity(&broker, 0).await;
    let terminate_broker = broker.clone();
    let terminate = tokio::spawn(async move {
        terminate_broker
            .terminate_runtime("session-1", generation)
            .await
    });
    timer.wait_for_sleep(Duration::from_secs(2)).await;
    assert_eq!(
        broker
            .external_command_capacity_for_test("session-1")
            .await
            .expect("still saturated"),
        0
    );
    timer.fire(Duration::from_secs(2)).await;
    terminate
        .await
        .expect("terminate task")
        .expect("deadline abort");
    release_tx.send(()).expect("release native resize");
    assert!(matches!(
        resize.await.expect("resize task"),
        Err(TerminalBrokerError::RuntimeTerminated)
    ));
    for thread in output_threads {
        assert!(matches!(
            thread.join().expect("output thread"),
            Err(TerminalBrokerError::RuntimeTerminated)
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activation_timeout_has_priority_over_a_saturated_external_queue() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin pending takeover");
    let activation_id = pending.activation_id.expect("activation id");
    let mut wakeups = broker.subscribe_wakeups();

    let release = broker
        .block_actor_for_test("session-1")
        .await
        .expect("block actor");
    let state_broker = broker.clone();
    let first_queued_state =
        tokio::spawn(async move { state_broker.broker_state("session-1").await });
    wait_for_external_capacity(&broker, TERMINAL_SESSION_ACTOR_CAPACITY - 1).await;
    let mut output_threads = Vec::new();
    for index in 0..(TERMINAL_SESSION_ACTOR_CAPACITY - 1) {
        let output_broker = broker.clone();
        output_threads.push(std::thread::spawn(move || {
            output_broker.process_output_blocking(
                "session-1",
                generation,
                vec![(index % 251) as u8],
            )
        }));
    }
    wait_for_external_capacity(&broker, 0).await;
    timer.fire(Duration::from_millis(16)).await;
    timer.fire(Duration::from_secs(5)).await;
    wait_for_activation_deadline_elapsed(&broker, &activation_id).await;
    release.send(()).expect("release actor");

    let wake = tokio::time::timeout(Duration::from_secs(5), wakeups.recv())
        .await
        .expect("wake was starved by saturated external queue")
        .expect("wake broadcast");
    assert_eq!(wake.runtime_generation, generation);
    let state = first_queued_state
        .await
        .expect("state task")
        .expect("queued state response");
    assert!(state.pending_activation.is_none());
    assert_eq!(state.owner_presentation_id.as_deref(), Some("owner"));
    for thread in output_threads {
        thread
            .join()
            .expect("output thread")
            .expect("output retained");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ack_submitted_before_deadline_wins_while_external_queue_is_full() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin pending takeover");
    let lease_epoch = pending.decision.lease_epoch;
    let activation_id = pending.activation_id.expect("activation id");

    let release = broker
        .block_actor_for_test("session-1")
        .await
        .expect("block actor");
    let mut output_threads = Vec::new();
    for index in 0..TERMINAL_SESSION_ACTOR_CAPACITY {
        let output_broker = broker.clone();
        output_threads.push(std::thread::spawn(move || {
            output_broker.process_output_blocking(
                "session-1",
                generation,
                vec![(index % 251) as u8],
            )
        }));
    }
    wait_for_external_capacity(&broker, 0).await;

    let ack_broker = broker.clone();
    let ack_activation_id = activation_id.clone();
    let ack = tokio::spawn(async move {
        ack_broker
            .ack_activation(TerminalActivationAckRequest {
                session_id: "session-1".to_string(),
                presentation_id: "takeover".to_string(),
                runtime_generation: generation,
                lease_epoch,
                activation_id: ack_activation_id,
            })
            .await
    });
    wait_for_ack_reservation(&broker, &activation_id).await;
    timer.fire(Duration::from_secs(5)).await;
    release.send(()).expect("release actor");

    let committed = ack
        .await
        .expect("ack task")
        .expect("ack after queue drains");
    assert_eq!(
        committed.decision.status,
        TerminalLeaseDecisionStatus::Accepted
    );
    assert_eq!(
        committed.broker_state.owner_presentation_id.as_deref(),
        Some("takeover")
    );
    assert!(committed.broker_state.pending_activation.is_none());
    for thread in output_threads {
        thread
            .join()
            .expect("output thread")
            .expect("output retained");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_one_of_two_predeadline_acks_keeps_the_other_protected() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin pending takeover");
    let activation_id = pending.activation_id.expect("activation id");
    let ack_request = TerminalActivationAckRequest {
        session_id: "session-1".to_string(),
        presentation_id: "takeover".to_string(),
        runtime_generation: generation,
        lease_epoch: pending.decision.lease_epoch,
        activation_id: activation_id.clone(),
    };

    let release = broker
        .block_actor_for_test("session-1")
        .await
        .expect("block actor");
    let mut output_threads = Vec::new();
    for index in 0..TERMINAL_SESSION_ACTOR_CAPACITY {
        let output_broker = broker.clone();
        output_threads.push(std::thread::spawn(move || {
            output_broker.process_output_blocking(
                "session-1",
                generation,
                vec![(index % 251) as u8],
            )
        }));
    }
    wait_for_external_capacity(&broker, 0).await;

    let first_broker = broker.clone();
    let first_request = ack_request.clone();
    let first = tokio::spawn(async move { first_broker.ack_activation(first_request).await });
    wait_for_ack_reservation_count(&broker, &activation_id, 1).await;
    let second_broker = broker.clone();
    let second = tokio::spawn(async move { second_broker.ack_activation(ack_request).await });
    wait_for_ack_reservation_count(&broker, &activation_id, 2).await;

    first.abort();
    assert!(first
        .await
        .expect_err("first ack must be cancelled")
        .is_cancelled());
    wait_for_ack_reservation_count(&broker, &activation_id, 1).await;
    timer.fire(Duration::from_secs(5)).await;
    release.send(()).expect("release actor");

    let committed = second
        .await
        .expect("second ack task")
        .expect("second ack after queue drains");
    assert_eq!(
        committed.decision.status,
        TerminalLeaseDecisionStatus::Accepted
    );
    assert_eq!(
        committed.broker_state.owner_presentation_id.as_deref(),
        Some("takeover")
    );
    assert!(committed.broker_state.pending_activation.is_none());
    for thread in output_threads {
        thread
            .join()
            .expect("output thread")
            .expect("output retained");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ack_submitted_after_deadline_is_rejected_as_stale() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin pending takeover");

    let release = broker
        .block_actor_for_test("session-1")
        .await
        .expect("block actor");
    let activation_id = pending.activation_id.expect("activation id");
    timer.fire(Duration::from_secs(5)).await;
    wait_for_activation_deadline_elapsed(&broker, &activation_id).await;
    let ack_broker = broker.clone();
    let ack = tokio::spawn(async move {
        ack_broker
            .ack_activation(TerminalActivationAckRequest {
                session_id: "session-1".to_string(),
                presentation_id: "takeover".to_string(),
                runtime_generation: generation,
                lease_epoch: pending.decision.lease_epoch,
                activation_id,
            })
            .await
    });
    release.send(()).expect("release actor");

    let rejected = ack.await.expect("ack task").expect("structured stale ack");
    assert_eq!(
        rejected.decision.status,
        TerminalLeaseDecisionStatus::Rejected
    );
    assert_eq!(
        rejected.decision.reason,
        Some(TerminalLeaseRejectionReason::StaleActivation)
    );
    assert_eq!(
        rejected.broker_state.owner_presentation_id.as_deref(),
        Some("owner")
    );
    assert!(rejected.broker_state.pending_activation.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postdeadline_ack_is_rejected_before_timeout_notification_is_processed() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin pending takeover");

    let release = broker
        .block_actor_for_test("session-1")
        .await
        .expect("block actor");
    timer.fire(Duration::from_secs(5)).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        broker.consume_control_notification_for_test("session-1"),
    )
    .await
    .expect("timeout notification was never published")
    .expect("consume timeout notification before actor sees it");
    let ack_broker = broker.clone();
    let ack = tokio::spawn(async move {
        ack_broker
            .ack_activation(TerminalActivationAckRequest {
                session_id: "session-1".to_string(),
                presentation_id: "takeover".to_string(),
                runtime_generation: generation,
                lease_epoch: pending.decision.lease_epoch,
                activation_id: pending.activation_id.expect("activation id"),
            })
            .await
    });
    wait_for_external_capacity(&broker, TERMINAL_SESSION_ACTOR_CAPACITY - 1).await;
    release.send(()).expect("release actor");

    let rejected = ack
        .await
        .expect("ack task")
        .expect("structured postdeadline ack");
    assert_eq!(
        rejected.decision.status,
        TerminalLeaseDecisionStatus::Rejected
    );
    assert_eq!(
        rejected.decision.reason,
        Some(TerminalLeaseRejectionReason::StaleActivation)
    );
    assert_eq!(
        rejected.broker_state.owner_presentation_id.as_deref(),
        Some("owner")
    );
    assert!(rejected.broker_state.pending_activation.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_ack_cannot_rollback_an_earlier_ack_held_before_send() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "takeover").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let pending = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "session-1".to_string(),
            presentation_id: "takeover".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: active.broker_state.lease_epoch,
        })
        .await
        .expect("begin pending takeover");
    let activation_id = pending.activation_id.expect("activation id");
    let ack_request = TerminalActivationAckRequest {
        session_id: "session-1".to_string(),
        presentation_id: "takeover".to_string(),
        runtime_generation: generation,
        lease_epoch: pending.decision.lease_epoch,
        activation_id: activation_id.clone(),
    };
    let (classified, release_early) = broker
        .gate_next_ack_after_classification_for_test("session-1")
        .await
        .expect("install ack classification gate");
    let early_broker = broker.clone();
    let early_request = ack_request.clone();
    let early = tokio::spawn(async move { early_broker.ack_activation(early_request).await });
    tokio::time::timeout(Duration::from_secs(5), classified)
        .await
        .expect("early ack never reached classification gate")
        .expect("early ack classification signal");
    wait_for_ack_reservation_count(&broker, &activation_id, 1).await;

    timer.fire(Duration::from_secs(5)).await;
    wait_for_activation_deadline_elapsed(&broker, &activation_id).await;
    let late = broker
        .ack_activation(ack_request)
        .await
        .expect("structured late ack");
    assert_eq!(late.decision.status, TerminalLeaseDecisionStatus::Rejected);
    assert_eq!(
        late.decision.reason,
        Some(TerminalLeaseRejectionReason::StaleActivation)
    );
    assert_eq!(
        late.broker_state
            .pending_activation
            .as_ref()
            .map(|pending| pending.activation_id.as_str()),
        Some(activation_id.as_str())
    );
    assert!(late.broker_state.owner_presentation_id.is_none());

    release_early.send(()).expect("release early ack");
    let committed = early
        .await
        .expect("early ack task")
        .expect("early ack after release");
    assert_eq!(
        committed.decision.status,
        TerminalLeaseDecisionStatus::Accepted
    );
    assert_eq!(
        committed.broker_state.owner_presentation_id.as_deref(),
        Some("takeover")
    );
    assert!(committed.broker_state.pending_activation.is_none());
}

#[tokio::test]
async fn terminal_session_spawn_geometry_uses_broker_deferred_then_canonical_state() {
    let broker = TerminalSessionBroker::default();
    broker
        .remember_deferred_geometry("deferred", "presentation", geometry(132, 41))
        .await
        .expect("deferred geometry");
    assert_eq!(
        broker
            .spawn_geometry("deferred")
            .await
            .expect("deferred lookup"),
        Some(geometry(132, 41))
    );

    let (runtime, _, _) = runtime();
    broker
        .start_or_replace_runtime("live", runtime, geometry(111, 37))
        .await
        .expect("live runtime");
    assert_eq!(
        broker
            .spawn_geometry("live")
            .await
            .expect("canonical lookup"),
        Some(geometry(111, 37))
    );
    assert_eq!(
        broker
            .spawn_geometry("unknown")
            .await
            .expect("empty lookup"),
        None
    );
}

#[tokio::test]
async fn terminal_session_kill_removes_runtime_while_pause_and_replace_keep_generation_rules() {
    let broker = TerminalSessionBroker::default();
    let (first_runtime, _, _) = runtime();
    let first = broker
        .start_or_replace_runtime("lifecycle", first_runtime, geometry(80, 24))
        .await
        .expect("first runtime");
    let paused = broker
        .pause_runtime("lifecycle", first)
        .await
        .expect("pause");
    assert_eq!(paused.runtime_state, TerminalRuntimeState::Paused);

    let (second_runtime, _, _) = runtime();
    let second = broker
        .start_or_replace_runtime("lifecycle", second_runtime, geometry(100, 30))
        .await
        .expect("resume replacement");
    assert_eq!(second, first + 1);
    broker
        .terminate_and_remove_runtime("lifecycle", second)
        .await
        .expect("kill removes runtime");
    assert_eq!(
        broker.broker_state("lifecycle").await,
        Err(TerminalBrokerError::SessionNotFound)
    );

    let (third_runtime, _, _) = runtime();
    let third = broker
        .start_or_replace_runtime("lifecycle", third_runtime, geometry(120, 40))
        .await
        .expect("recreate killed runtime");
    assert_eq!(
        third,
        second + 1,
        "runtime removal must retain a generation tombstone"
    );

    broker
        .register_presentation(
            TerminalPresentationRegistration {
                presentation_id: "recreated-presentation".to_string(),
                session_id: "lifecycle".to_string(),
                client_kind: TerminalClientKind::Desktop,
                desired_geometry: Some(geometry(120, 40)),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Mounted,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: 0,
            },
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("register recreated presentation");
    broker
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: "lifecycle".to_string(),
            consumer_id: "recreated-consumer".to_string(),
            client_kind: TerminalClientKind::Desktop,
            runtime_generation: third,
        })
        .await
        .expect("subscribe recreated consumer");

    let stale_output =
        broker.process_output_blocking("lifecycle", second, b"late old-generation output".to_vec());
    assert_eq!(
        stale_output,
        Err(TerminalBrokerError::StaleRuntimeGeneration {
            expected: third,
            received: second,
        })
    );
    assert_eq!(
        broker
            .unregister_presentation("lifecycle", "recreated-presentation", second)
            .await,
        Err(TerminalBrokerError::StaleRuntimeGeneration {
            expected: third,
            received: second,
        })
    );
    assert_eq!(
        broker
            .subscribe(TerminalEventSubscriptionRequest {
                session_id: "lifecycle".to_string(),
                consumer_id: "late-old-consumer".to_string(),
                client_kind: TerminalClientKind::Remote,
                runtime_generation: second,
            })
            .await,
        Err(TerminalBrokerError::StaleRuntimeGeneration {
            expected: third,
            received: second,
        })
    );

    let presentation = broker
        .report_presentation_viewport(TerminalPresentationViewportRequest {
            session_id: "lifecycle".to_string(),
            presentation_id: "recreated-presentation".to_string(),
            runtime_generation: third,
            cols: 120,
            rows: 40,
        })
        .await
        .expect("new presentation survives stale requests");
    assert_eq!(presentation.presentation_id, "recreated-presentation");
    let acknowledgements = broker
        .consumer_acknowledgements("lifecycle")
        .await
        .expect("new consumer survives stale requests");
    assert_eq!(acknowledgements.get("recreated-consumer"), Some(&0));
    assert!(!acknowledgements.contains_key("late-old-consumer"));
    assert!(!broker
        .snapshot("lifecycle")
        .await
        .expect("replacement snapshot")
        .visible_grid
        .contains("late old-generation output"));
}

#[tokio::test]
async fn terminal_session_privileged_input_is_serialized_and_survives_desktop_ownership() {
    let broker = TerminalSessionBroker::default();
    let (runtime, mut input_rx, _) = runtime();
    let generation = broker
        .start_or_replace_runtime("privileged", runtime, geometry(80, 24))
        .await
        .expect("runtime");
    broker
        .register_presentation(
            TerminalPresentationRegistration {
                presentation_id: "desktop-owner".to_string(),
                session_id: "privileged".to_string(),
                client_kind: TerminalClientKind::Desktop,
                desired_geometry: Some(geometry(100, 30)),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Mounted,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: 0,
            },
            TerminalClientIdentity::trusted_desktop(),
        )
        .await
        .expect("register owner");
    let begin = broker
        .begin_activation(TerminalActivationBeginRequest {
            session_id: "privileged".to_string(),
            presentation_id: "desktop-owner".to_string(),
            runtime_generation: generation,
            observed_lease_epoch: 0,
        })
        .await
        .expect("begin");
    broker
        .ack_activation(TerminalActivationAckRequest {
            session_id: "privileged".to_string(),
            presentation_id: "desktop-owner".to_string(),
            runtime_generation: generation,
            lease_epoch: begin.decision.lease_epoch,
            activation_id: begin.activation_id.expect("activation id"),
        })
        .await
        .expect("ack");

    broker
        .send_privileged_input("privileged", b"system-control".to_vec())
        .await
        .expect("privileged input");
    assert_eq!(
        input_rx.recv().await.as_deref(),
        Some(b"system-control".as_slice())
    );
}

#[tokio::test]
async fn terminal_session_pause_drops_runtime_handles_and_closes_input_channel() {
    let broker = TerminalSessionBroker::default();
    let (runtime, mut input_rx, _) = runtime();
    let generation = broker
        .start_or_replace_runtime("paused-handles", runtime, geometry(80, 24))
        .await
        .expect("runtime");

    broker
        .pause_runtime("paused-handles", generation)
        .await
        .expect("pause");

    assert_eq!(
        input_rx.recv().await,
        None,
        "pause must drop the actor's sole sender"
    );
    assert_eq!(
        broker
            .send_privileged_input("paused-handles", b"blocked".to_vec())
            .await,
        Err(TerminalBrokerError::RuntimeUnavailable)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_session_legacy_read_cursor_is_generation_scoped() {
    let broker = Arc::new(TerminalSessionBroker::default());
    let (first_runtime, _, _) = runtime();
    let first_generation = broker
        .start_or_replace_runtime("reused", first_runtime, geometry(80, 24))
        .await
        .expect("first runtime");
    process_output_for_session(broker.clone(), "reused", first_generation, b"first")
        .expect("first output");
    assert_eq!(
        broker
            .read_legacy_output("reused", None, false)
            .await
            .expect("first read")
            .as_deref(),
        Some("first")
    );
    assert_eq!(
        broker
            .read_legacy_output("reused", None, false)
            .await
            .expect("drained read"),
        None
    );

    let (second_runtime, _, _) = runtime();
    let second_generation = broker
        .start_or_replace_runtime("reused", second_runtime, geometry(100, 30))
        .await
        .expect("replacement runtime");
    process_output_for_session(broker.clone(), "reused", second_generation, b"second")
        .expect("second output");
    assert_eq!(
        broker
            .read_legacy_output("reused", None, false)
            .await
            .expect("replacement read")
            .as_deref(),
        Some("second")
    );
}

#[tokio::test]
async fn terminal_session_replacement_lifecycle_notifications_never_regress_generation() {
    let broker = TerminalSessionBroker::default();
    let mut lifecycle = broker.subscribe_lifecycle();
    let (first_runtime, _, _) = runtime();
    let first = broker
        .start_or_replace_runtime("ordered-lifecycle", first_runtime, geometry(80, 24))
        .await
        .expect("first runtime");
    assert_eq!(
        lifecycle.recv().await.expect("started").runtime_generation,
        first
    );
    let (second_runtime, _, _) = runtime();
    let second = broker
        .start_or_replace_runtime("ordered-lifecycle", second_runtime, geometry(100, 30))
        .await
        .expect("second runtime");
    let replaced_old = lifecycle.recv().await.expect("old replacement");
    let replaced_new = lifecycle.recv().await.expect("new replacement");
    assert!(replaced_old.runtime_generation <= replaced_new.runtime_generation);
    assert_eq!(replaced_new.runtime_generation, second);
}

fn process_output_for_session(
    broker: Arc<TerminalSessionBroker>,
    session_id: &'static str,
    runtime_generation: u64,
    bytes: &'static [u8],
) -> Result<(), TerminalBrokerError> {
    std::thread::spawn(move || {
        broker.process_output_blocking(session_id, runtime_generation, bytes.to_vec())
    })
    .join()
    .expect("reader thread")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseding_activations_keep_only_the_current_timeout_signal() {
    let timer = Arc::new(ManualTimer::default());
    let (broker, generation) = start(timer.clone()).await;
    register_desktop(&broker, "owner").await;
    register_desktop(&broker, "one").await;
    register_desktop(&broker, "two").await;
    let active = activate(&broker, "owner", generation, 0).await;
    let mut observed_lease_epoch = active.broker_state.lease_epoch;
    let mut current_activation_id = None;

    for index in 0..64 {
        let pending = broker
            .begin_activation(TerminalActivationBeginRequest {
                session_id: "session-1".to_string(),
                presentation_id: if index % 2 == 0 { "one" } else { "two" }.to_string(),
                runtime_generation: generation,
                observed_lease_epoch,
            })
            .await
            .expect("superseding begin");
        assert_eq!(
            pending.decision.status,
            TerminalLeaseDecisionStatus::Accepted
        );
        observed_lease_epoch = pending.decision.lease_epoch;
        current_activation_id = pending.activation_id;
    }

    wait_for_live_sleep_count(&timer, Duration::from_secs(5), 1).await;
    assert_eq!(
        broker
            .activation_control_slots_for_test("session-1")
            .await
            .expect("control diagnostic"),
        1
    );
    let state = broker
        .broker_state("session-1")
        .await
        .expect("actor remains responsive");
    assert_eq!(
        state
            .pending_activation
            .as_ref()
            .map(|pending| pending.activation_id.as_str()),
        current_activation_id.as_deref()
    );

    timer.fire(Duration::from_secs(5)).await;
    let rolled_back = broker
        .broker_state("session-1")
        .await
        .expect("current timeout was not starved");
    assert!(rolled_back.pending_activation.is_none());
    assert_eq!(rolled_back.owner_presentation_id.as_deref(), Some("owner"));
}
