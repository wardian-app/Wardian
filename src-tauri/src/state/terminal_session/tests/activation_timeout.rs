use super::*;

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
    // Releasing the timer does not await timeout publication or actor rollback.
    let rolled_back = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = broker
                .broker_state("session-1")
                .await
                .expect("actor remains responsive while timeout settles");
            if state.pending_activation.is_none() {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("current timeout was not starved");
    assert!(rolled_back.pending_activation.is_none());
    assert_eq!(rolled_back.owner_presentation_id.as_deref(), Some("owner"));
}
