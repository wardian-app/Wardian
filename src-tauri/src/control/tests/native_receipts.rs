//! Live delivery must honor provider acceptance boundaries independently of PTY writes.
use super::*;

#[tokio::test]
async fn pi_required_receipt_rejects_legacy_runtime_without_extension_before_writing() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    insert_test_agent(&state, "pi-receipt-agent", "PiReceipt", "Coder").await;
    {
        let mut agents = state.agents.lock().await;
        let agent = agents.get_mut("pi-receipt-agent").unwrap();
        // This test owns no process; do not retain the generic fixture PID.
        agent.process_id = None;
        let mut config = agent.config.lock().unwrap();
        config.provider = "pi".to_string();
        config.fresh_provider_session_id = Some(uuid::Uuid::new_v4().to_string());
        *agent.current_status.lock().unwrap() = "Idle".to_string();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    install_test_terminal_runtime(&state, "pi-receipt-agent", tx).await;
    let generation = state.agents.lock().await["pi-receipt-agent"]
        .runtime_generation
        .unwrap();
    let native_ack = state
        .terminal_sessions
        .native_write_receipts_enabled("pi-receipt-agent")
        .await
        .unwrap();
    let mut request = crate::delivery::LiveSurfacePromptRequest::message(
        "pi-receipt-agent",
        "plain receipt probe",
    );
    request.require_provider_turn_receipt = true;
    let result = crate::delivery::submit_live_surface_prompt(None, &state, request).await;
    let mut written_bytes = 0;
    while let Ok(bytes) = rx.try_recv() {
        written_bytes += bytes.len();
    }
    // Release the mock actor before either red or green assertions.
    state
        .terminal_sessions
        .terminate_and_remove_runtime("pi-receipt-agent", generation)
        .await
        .unwrap();
    state.agents.lock().await.remove("pi-receipt-agent");

    assert!(!native_ack, "precondition: legacy writer has no PTY ack");
    assert_eq!(
        written_bytes, 0,
        "a required Pi receipt must check extension capability before any payload or submit bytes"
    );
    let error = result.expect_err("missing Pi extension must reject required receipt");
    assert!(error.retry_safe);
    let detail = error.detail.expect("structured pre-write failure");
    assert_eq!(
        detail.error.as_ref().map(|error| error.code.as_str()),
        Some("pi_receipt_unavailable")
    );
    assert_eq!(
        detail.delivery_phase.as_deref(),
        Some("receipt_capability_unavailable")
    );
}

#[tokio::test]
async fn native_codex_delivery_waits_for_provider_applied_payload() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
    {
        let agents = state.agents.lock().await;
        let agent = agents.get("agent-1").unwrap();
        agent.config.lock().unwrap().provider = "codex".to_string();
        *agent.current_status.lock().unwrap() = "Idle".to_string();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    install_test_terminal_runtime_with_write_receipts(&state, "agent-1", tx).await;
    let generation = state.agents.lock().await["agent-1"]
        .runtime_generation
        .expect("registered test runtime generation");
    assert!(
        record_provider_ready_evidence(
            &state,
            "agent-1",
            generation,
            ProviderReadyEvidence::PromptDetected,
        )
        .await,
        "readiness evidence must be recorded for the registered runtime generation"
    );

    let delivery = deliver_message_to_target(
        None,
        &state,
        "CoderOne",
        "hello",
        None,
        MessageInputMode::Message,
        QueuePolicy::QueueIfBusy,
        None,
        None,
        false,
    );
    tokio::pin!(delivery);

    let payload = tokio::select! {
        request = rx.recv() => request.expect("payload write request"),
        result = &mut delivery => panic!("delivery completed before payload write: {result:?}"),
    };
    assert_eq!(payload.bytes, b"\x1b[200~hello\x1b[201~".to_vec());
    payload.completion.send(Ok(())).expect("payload receipt");

    // The PTY receipt alone must not release Return. Codex's repaint is the
    // provider-owned proof that its composer consumed the paste.
    crate::delivery::codex_composer::tests::record_active_composer_repaint(
        &state,
        "agent-1",
        b"\r\n\xe2\x80\xba hello",
    )
    .await;

    let submit = tokio::select! {
        request = rx.recv() => request.expect("submit write request"),
        result = &mut delivery => panic!("delivery completed before submit write: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {
            panic!("Codex submit did not follow provider-applied payload evidence")
        }
    };
    assert_eq!(submit.bytes, b"\r".to_vec());
    submit.completion.send(Ok(())).expect("submit receipt");

    crate::manager::record_agent_turn_started_for_watch(&state, "agent-1").await;
    let delivery = delivery.await.expect("delivered after provider receipt");

    assert_eq!(delivery[0].delivery_state, "provider_accepted");
    assert_eq!(delivery[0].delivery_phase.as_deref(), Some("turn_started"));
}
