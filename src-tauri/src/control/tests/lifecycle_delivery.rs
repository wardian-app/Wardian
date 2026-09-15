// Regression coverage for lifecycle contention at the live delivery boundary.
fn active_test_lease(
    agent_id: &str,
    resume_session: &str,
) -> wardian_core::conversation_lease::ConversationLease {
    let now = chrono::Utc::now();
    wardian_core::conversation_lease::ConversationLease {
        agent_id: agent_id.to_string(),
        provider: "mock".to_string(),
        resume_session: resume_session.to_string(),
        owner_kind: "message_delivery".to_string(),
        owner_id: "lifecycle-delivery-test".to_string(),
        acquisition_id: "lifecycle-delivery-test-acquisition".to_string(),
        owner_node_id: None,
        mode: "background_resume".to_string(),
        started_at: now.to_rfc3339(),
        heartbeat_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_delivery_waits_for_transient_lifecycle_contention() {
    let _home = TestWardianHome::new_async().await;
    let state = std::sync::Arc::new(AppState::new());
    insert_test_agent(&state, "receiver", "Receiver", "Test").await;
    {
        let agents = state.agents.lock().await;
        *agents
            .get("receiver")
            .expect("receiver")
            .current_status
            .lock()
            .expect("status") = "Idle".to_string();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    install_test_terminal_runtime(&state, "receiver", tx).await;

    let lifecycle_guard = state.lock_agent_lifecycle("receiver").await;
    let delivery_state = std::sync::Arc::clone(&state);
    let delivery = tokio::spawn(async move {
        deliver_prompt_to_agent(
            None,
            &delivery_state,
            "Receiver",
            "contention prompt",
            MessageInputMode::Message,
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(
        !delivery.is_finished(),
        "delivery must wait on the lifecycle gate"
    );

    drop(lifecycle_guard);
    let detail = delivery
        .await
        .expect("delivery task")
        .expect("transient lifecycle contention must not reject delivery");
    assert_eq!(detail.delivery_state, "submit_sent_unconfirmed");
    assert_eq!(
        rx.recv().await.expect("submitted payload"),
        b"contention prompt".to_vec()
    );
    assert_eq!(rx.recv().await.expect("submitted return"), b"\r".to_vec());
}

#[tokio::test(flavor = "current_thread")]
async fn task_composer_rejects_after_input_pause_without_writing_live_runtime() {
    let _home = TestWardianHome::new_async().await;
    let state = std::sync::Arc::new(AppState::new());
    insert_test_agent(&state, "receiver", "Receiver", "Test").await;
    {
        let agents = state.agents.lock().await;
        *agents
            .get("receiver")
            .expect("receiver")
            .current_status
            .lock()
            .expect("status") = "Idle".to_string();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    install_test_terminal_runtime(&state, "receiver", tx).await;
    let generation = state
        .terminal_sessions
        .broker_state("receiver")
        .await
        .expect("runtime state")
        .runtime_generation;
    state
        .terminal_sessions
        .pause_input_sender("receiver", generation)
        .await
        .expect("pause input sender");

    let error = deliver_prompt_to_agent(
        None,
        &state,
        "Receiver",
        "composer must not write while input is paused",
        MessageInputMode::Message,
    )
    .await
    .expect_err("paused task composer must fail before writing");
    assert!(error.to_string().contains("RuntimeUnavailable"));
    assert!(
        rx.try_recv().is_err(),
        "task-composer delivery must emit no payload or submit bytes"
    );

    state
        .terminal_sessions
        .send_privileged_input("receiver", b"explicit-control".to_vec())
        .await
        .expect("explicit privileged control remains available");
    assert_eq!(
        rx.recv().await.expect("explicit control write"),
        b"explicit-control".to_vec()
    );
}

#[tokio::test]
async fn live_delivery_rejects_an_actual_active_conversation_lease() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    insert_test_agent(&state, "leased", "Leased", "Test").await;
    let lease = active_test_lease("leased", "provider-session");
    wardian_core::conversation_lease::acquire_lease(lease.clone(), &lease.started_at)
        .expect("active lease");

    let result = deliver_prompt_to_agent(
        None,
        &state,
        "Leased",
        "blocked prompt",
        MessageInputMode::Message,
    )
    .await
    .expect_err("an active lease must remain a delivery rejection");
    assert!(result.to_string().contains("conversation_leased"));
    wardian_core::conversation_lease::release_lease_owner_persisted(&lease.owner())
        .expect("release test lease");
}

#[tokio::test(flavor = "current_thread")]
async fn delivery_fails_closed_when_the_target_incarnation_is_replaced_while_waiting() {
    let _home = TestWardianHome::new_async().await;
    let state = std::sync::Arc::new(AppState::new());
    insert_test_agent(&state, "receiver", "Receiver", "Test").await;
    {
        let agents = state.agents.lock().await;
        *agents
            .get("receiver")
            .expect("receiver")
            .current_status
            .lock()
            .expect("status") = "Idle".to_string();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    install_test_terminal_runtime(&state, "receiver", tx).await;

    let lifecycle_guard = state.lock_agent_lifecycle("receiver").await;
    let delivery_state = std::sync::Arc::clone(&state);
    let delivery = tokio::spawn(async move {
        deliver_prompt_to_agent(
            None,
            &delivery_state,
            "Receiver",
            "replacement prompt",
            MessageInputMode::Message,
        )
        .await
    });
    tokio::task::yield_now().await;

    let replacement_config = {
        let agents = state.agents.lock().await;
        let config = agents
            .get("receiver")
            .expect("receiver")
            .config
            .lock()
            .expect("config")
            .clone();
        config
    };
    let replacement_config = std::sync::Arc::new(std::sync::Mutex::new(replacement_config));
    state
        .agents
        .lock()
        .await
        .get_mut("receiver")
        .expect("receiver")
        .config = replacement_config;

    drop(lifecycle_guard);
    let result = delivery
        .await
        .expect("delivery task")
        .expect_err("replaced target must fail closed");
    assert!(result.to_string().contains("target_replaced"));
    assert!(
        rx.try_recv().is_err(),
        "stale delivery must not write to successor"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn codex_task_started_fixture_reaches_the_provider_receipt_waiter() {
    use crate::providers::codex::CodexProvider;
    use wardian_core::models::{AgentEvent, AgentProvider};

    let state = AppState::new();
    insert_test_agent(&state, "receiver", "Receiver", "Test").await;
    let provider = CodexProvider::new();
    let line = include_str!("../../providers/fixtures/codex-real-delivery-mirror.jsonl")
        .lines()
        .next()
        .expect("retained Codex delivery fixture");
    let event = provider.parse_output(line).expect("task_started event");
    assert!(matches!(
        &event,
        AgentEvent::TurnStarted { turn_id }
            if turn_id.as_str() == "01a0a1e5-2f6d-7530-bebc-f44c8c299bb7"
    ));
    assert!(
        crate::manager::ProviderStatusEventPolicy::PreserveActionRequired
            .confirms_turn_started(&event)
    );

    let cursor = provider_turn_start_cursor(&state, "receiver")
        .await
        .expect("provider receipt cursor");
    let wait = wait_for_provider_turn_started_after_submit(&state, "receiver", &cursor);
    tokio::pin!(wait);
    crate::manager::record_agent_turn_started_for_watch(&state, "receiver").await;
    wait.await
        .expect("a parsed Codex lifecycle start satisfies the real receipt waiter");
}
