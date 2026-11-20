use super::super::test_support::TestWardianHome;
use super::*;
use crate::utils::delivery_transaction::{
    select_delivery_input_sink, AutomatedInputWriteFence, AutomatedTerminalInputSink,
    BrokerTerminalInputSink, TerminalInputSink,
};
use wardian_core::control::ReplyStatus;

async fn agent(state: &AppState, id: &str) {
    let agent = super::super::tests::test_agent(id, id, "Test");
    *agent.current_status.lock().unwrap() = "Off".into();
    state.agents.lock().await.insert(id.into(), agent);
}

async fn automated_input_fixture() -> (
    AppState,
    tokio::sync::mpsc::Receiver<Vec<u8>>,
    AutomatedInputWriteFence,
    std::sync::Arc<std::sync::Mutex<wardian_core::models::AgentConfig>>,
) {
    let state = AppState::new();
    agent(&state, "receiver").await;
    let (config_identity, status_identity) = {
        let agents = state.agents.lock().await;
        let receiver = agents.get("receiver").expect("fixture recipient");
        (receiver.config.clone(), receiver.current_status.clone())
    };
    {
        let mut config = config_identity.lock().expect("fixture config");
        config.provider = "codex".into();
        config.resume_session = Some("thread-original".into());
        config.is_off = false;
    }
    *status_identity.lock().expect("fixture status") = "Idle".into();
    let provider_input_generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(8);
    let terminal_runtime_generation = state
        .terminal_sessions
        .start_or_replace_runtime(
            "receiver",
            crate::state::terminal_session::TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
            wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
        )
        .await
        .expect("fixture terminal runtime");
    let fence = AutomatedInputWriteFence {
        provider: "codex".into(),
        provider_conversation_id: "thread-original".into(),
        provider_input_generation,
        terminal_runtime_generation,
        agent_config_identity: config_identity.clone(),
    };
    (state, input_rx, fence, config_identity)
}

#[tokio::test]
async fn automated_fallback_writes_unchanged_target_once() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);

    sink.send_bytes(b"continuation".to_vec())
        .await
        .expect("unchanged conversation accepts one write");

    assert_eq!(
        input_rx.recv().await.as_deref(),
        Some(b"continuation".as_slice())
    );
    assert!(input_rx.try_recv().is_err(), "the boundary writes once");
}

#[tokio::test]
async fn automated_fallback_rejects_new_session_after_preflight_without_writing() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, config_identity) = automated_input_fixture().await;
    config_identity
        .lock()
        .expect("fixture config")
        .resume_session = Some("thread-after-new-session".into());
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);

    let error = sink
        .send_bytes(b"must stay on original thread".to_vec())
        .await
        .expect_err("new session fences the old reply");

    assert!(error.retry_safe);
    assert!(
        input_rx.try_recv().is_err(),
        "no bytes reach the replacement"
    );
}

#[tokio::test]
async fn automated_fallback_rejects_provider_generation_change_after_preflight_without_writing() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Booting, None)
        .await;
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);

    let error = sink
        .send_bytes(b"must stay on the original provider generation".to_vec())
        .await
        .expect_err("a new provider input generation fences the old reply");

    assert!(error.retry_safe);
    assert!(
        input_rx.try_recv().is_err(),
        "no bytes reach the new generation"
    );
}

#[tokio::test]
async fn automated_fallback_rejects_recreated_agent_after_clear_without_writing() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    agent(&state, "receiver").await;
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);

    let error = sink
        .send_bytes(b"must stay on deleted incarnation".to_vec())
        .await
        .expect_err("recreated agent incarnation fences the old reply");

    assert!(error.retry_safe);
    assert!(input_rx.try_recv().is_err(), "no bytes reach the new agent");
}

#[tokio::test]
async fn automated_fallback_rejects_lease_acquired_after_preflight_without_writing() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    let now = chrono::Utc::now();
    let started_at = now.to_rfc3339();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: "receiver".into(),
        provider: "codex".into(),
        resume_session: "thread-original".into(),
        owner_kind: "test".into(),
        owner_id: "lease-acquired-after-preflight".into(),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: None,
        mode: "background_resume".into(),
        started_at: started_at.clone(),
        heartbeat_at: started_at.clone(),
        expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    assert_eq!(
        wardian_core::conversation_lease::try_acquire_lease(lease, &started_at).unwrap(),
        wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired
    );
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);

    let error = sink
        .send_bytes(b"must not overlap lease".to_vec())
        .await
        .expect_err("the authoritative lease blocks fallback input");

    assert!(error.retry_safe);
    assert!(input_rx.try_recv().is_err(), "lease owns the conversation");
}

#[tokio::test]
async fn automated_fallback_respects_writer_pause_without_writing() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    state
        .terminal_sessions
        .pause_input_sender("receiver", fence.terminal_runtime_generation)
        .await
        .expect("pause the composer at the actor boundary");
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);

    let error = sink
        .send_bytes(b"must wait until the writer resumes".to_vec())
        .await
        .expect_err("the actor rejects automated input while paused");

    assert!(error.retry_safe);
    assert!(input_rx.try_recv().is_err(), "paused input writes no bytes");
    assert!(wardian_core::conversation_lease::load_leases()
        .iter()
        .all(|lease| lease.agent_id != "receiver"
            || lease.owner_kind != "message_delivery"
            || !lease.owner_id.starts_with("automatic-input:")));
}

#[tokio::test]
async fn ordinary_manual_composer_input_keeps_its_existing_write_path() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    let sink = BrokerTerminalInputSink::new_for_composer(
        state.terminal_sessions.clone(),
        "receiver",
        fence.terminal_runtime_generation,
    );

    sink.send_bytes(b"manual input".to_vec())
        .await
        .expect("manual composer input remains available");

    assert_eq!(
        input_rx.recv().await.as_deref(),
        Some(b"manual input".as_slice())
    );
    assert!(input_rx.try_recv().is_err());
}

#[tokio::test]
async fn delayed_automated_write_allows_unrelated_generation_and_lease_heartbeat() {
    let _home = TestWardianHome::new_async().await;
    let state = std::sync::Arc::new(AppState::new());
    agent(&state, "receiver").await;
    let (config_identity, status_identity) = {
        let agents = state.agents.lock().await;
        let receiver = agents.get("receiver").expect("fixture recipient");
        (receiver.config.clone(), receiver.current_status.clone())
    };
    {
        let mut config = config_identity.lock().expect("fixture config");
        config.provider = "codex".into();
        config.resume_session = Some("thread-original".into());
        config.is_off = false;
    }
    *status_identity.lock().expect("fixture status") = "Idle".into();
    let provider_input_generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;

    let now = chrono::Utc::now();
    let heartbeat_owner = wardian_core::conversation_lease::ConversationLease {
        agent_id: "unrelated-agent".into(),
        provider: "claude".into(),
        resume_session: "unrelated-conversation".into(),
        owner_kind: "test_heartbeat".into(),
        owner_id: "unrelated-lease-renewal".into(),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: None,
        mode: "background_resume".into(),
        started_at: now.to_rfc3339(),
        heartbeat_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::minutes(10)).to_rfc3339(),
    };
    assert_eq!(
        wardian_core::conversation_lease::try_acquire_lease(
            heartbeat_owner.clone(),
            &now.to_rfc3339(),
        )
        .expect("unrelated lease admission"),
        wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired
    );

    let (input_tx, mut input_rx) =
        tokio::sync::mpsc::channel::<crate::state::terminal_session::NativeTerminalWriteRequest>(1);
    let terminal_runtime_generation = state
        .terminal_sessions
        .start_or_replace_runtime(
            "receiver",
            crate::state::terminal_session::TerminalRuntimeHandles::new_with_write_ack(
                input_tx,
                |_| Ok(()),
            ),
            wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
        )
        .await
        .expect("fixture terminal runtime");
    let fence = AutomatedInputWriteFence {
        provider: "codex".into(),
        provider_conversation_id: "thread-original".into(),
        provider_input_generation,
        terminal_runtime_generation,
        agent_config_identity: config_identity,
    };
    let sink = AutomatedTerminalInputSink::new(&state, "receiver", fence);
    let send = sink.send_bytes(b"delayed provider write".to_vec());
    tokio::pin!(send);
    let writer_request = tokio::select! {
        result = &mut send => panic!("writer returned before its completion receipt: {result:?}"),
        request = input_rx.recv() => request.expect("automated writer request"),
    };
    assert_eq!(
        writer_request.bytes.as_slice(),
        b"delayed provider write".as_slice()
    );

    let interactions = state.interactions.clone();
    let (same_session_started_tx, same_session_started_rx) = tokio::sync::oneshot::channel();
    let mut same_session_generation_update = tokio::spawn(async move {
        let _ = same_session_started_tx.send(());
        interactions
            .start_provider_input_generation("receiver", ProviderInputReadiness::Booting, None)
            .await
            .generation
    });
    same_session_started_rx
        .await
        .expect("same-session generation transition started");

    let interactions = state.interactions.clone();
    let mut generation_update = tokio::spawn(async move {
        let started = interactions
            .start_provider_input_generation(
                "unrelated-generation-agent",
                ProviderInputReadiness::Booting,
                None,
            )
            .await;
        let current = interactions
            .current_provider_input_generation("unrelated-generation-agent")
            .await;
        (started.generation, current)
    });
    let lease_owner = heartbeat_owner.owner();
    let mut heartbeat = tokio::task::spawn_blocking(move || {
        let now = chrono::Utc::now();
        wardian_core::conversation_lease::renew_lease_owner_persisted(
            &lease_owner,
            &now.to_rfc3339(),
            &(now + chrono::Duration::minutes(10)).to_rfc3339(),
        )
    });
    let (
        same_session_while_write_pending,
        generation_while_write_pending,
        heartbeat_while_write_pending,
    ) = tokio::join!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            &mut same_session_generation_update,
        ),
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut generation_update),
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut heartbeat),
    );
    let same_session_stayed_fenced = same_session_while_write_pending.is_err();
    let generation_progressed_while_pending = generation_while_write_pending.is_ok();
    let heartbeat_progressed_while_pending = heartbeat_while_write_pending.is_ok();

    writer_request
        .completion
        .send(Ok(()))
        .expect("release the delayed native writer");
    send.await.expect("automated write completes after receipt");
    assert!(wardian_core::conversation_lease::load_leases()
        .iter()
        .all(|lease| lease.agent_id != "receiver"));

    let generation = match generation_while_write_pending {
        Ok(result) => result.expect("unrelated generation update task"),
        Err(_) => generation_update
            .await
            .expect("unrelated generation update task"),
    };
    let heartbeat = match heartbeat_while_write_pending {
        Ok(result) => result.expect("unrelated lease heartbeat task"),
        Err(_) => heartbeat.await.expect("unrelated lease heartbeat task"),
    };
    let same_session_generation = match same_session_while_write_pending {
        Ok(result) => result.expect("same-session generation transition task"),
        Err(_) => same_session_generation_update
            .await
            .expect("same-session generation transition task"),
    };
    assert_eq!(generation, (1, Some(1)));
    assert_eq!(same_session_generation, 2);
    assert!(heartbeat.expect("persisted lease heartbeat"));
    assert!(
        generation_progressed_while_pending,
        "another agent's generation start and current-generation read must progress while this writer waits"
    );
    assert!(
        heartbeat_progressed_while_pending,
        "an unrelated persisted lease heartbeat must progress while this writer waits"
    );
    assert!(
        same_session_stayed_fenced,
        "the admitted provider generation remains fenced until the delayed write commits"
    );
}

#[tokio::test]
async fn uncertain_automated_writer_receipt_retains_its_conversation_lease() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "receiver").await;
    let (config_identity, status_identity) = {
        let agents = state.agents.lock().await;
        let receiver = agents.get("receiver").expect("fixture recipient");
        (receiver.config.clone(), receiver.current_status.clone())
    };
    {
        let mut config = config_identity.lock().expect("fixture config");
        config.provider = "codex".into();
        config.resume_session = Some("thread-original".into());
        config.is_off = false;
    }
    *status_identity.lock().expect("fixture status") = "Idle".into();
    let provider_input_generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(1);
    let terminal_runtime_generation = state
        .terminal_sessions
        .start_or_replace_runtime(
            "receiver",
            crate::state::terminal_session::TerminalRuntimeHandles::new_with_write_ack(
                input_tx,
                |_| Ok(()),
            ),
            wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
        )
        .await
        .expect("fixture terminal runtime");
    let sink = AutomatedTerminalInputSink::new(
        &state,
        "receiver",
        AutomatedInputWriteFence {
            provider: "codex".into(),
            provider_conversation_id: "thread-original".into(),
            provider_input_generation,
            terminal_runtime_generation,
            agent_config_identity: config_identity,
        },
    );
    let send = sink.send_bytes(b"possibly partial write".to_vec());
    tokio::pin!(send);
    let writer_request = tokio::select! {
        result = &mut send => panic!("writer returned before its completion receipt: {result:?}"),
        request = input_rx.recv() => request.expect("automated writer request"),
    };
    writer_request
        .completion
        .send(Err("writer receipt is uncertain".to_string()))
        .expect("report an uncertain native write result");

    let error = send.await.expect_err("uncertain writer result is surfaced");
    assert!(!error.retry_safe);
    assert!(wardian_core::conversation_lease::load_leases()
        .iter()
        .any(|lease| lease.agent_id == "receiver"
            && lease.owner_kind == "message_delivery"
            && lease.owner_id.starts_with("automatic-input:")
            && lease.mode == "lifecycle_transition"));
}

#[tokio::test]
async fn cancelled_lease_acquisition_releases_unhanded_reservation() {
    let _home = TestWardianHome::new_async().await;
    let admission = crate::state::terminal_session::AutomatedConversationLeaseAdmission::new(
        "receiver",
        "codex",
        "thread-original",
    );
    let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let mut acquisition = tokio::spawn(async move {
        admission
            .acquire_with_handoff_hook(move || {
                let _ = acquired_tx.send(());
                let _ = release_rx.recv();
            })
            .await
    });

    let acquired_before_cancel = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), acquired_rx).await,
        Ok(Ok(()))
    );
    acquisition.abort();
    let cancelled_before_handoff = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), &mut acquisition).await,
        Ok(Err(error)) if error.is_cancelled()
    );

    // Always release the blocking worker before assertions, including timeout
    // and cancellation failures, so a broken test cannot strand a pool thread.
    let gate_released = release_tx.send(()).is_ok();
    let no_retained_lease = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match wardian_core::conversation_lease::load_leases_checked() {
                Ok(leases)
                    if !leases.iter().any(|lease| {
                        lease.agent_id == "receiver"
                            && lease.owner_kind == "message_delivery"
                            && lease.owner_id.starts_with("automatic-input:")
                    }) =>
                {
                    return true;
                }
                Err(_) => return false,
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap_or(false);

    if !no_retained_lease {
        if let Ok(leases) = wardian_core::conversation_lease::load_leases_checked() {
            for lease in leases.iter().filter(|lease| {
                lease.agent_id == "receiver"
                    && lease.owner_kind == "message_delivery"
                    && lease.owner_id.starts_with("automatic-input:")
            }) {
                let _ =
                    wardian_core::conversation_lease::release_lease_owner_persisted(&lease.owner());
            }
        }
    }

    assert!(
        acquired_before_cancel,
        "the worker acquired before cancellation"
    );
    assert!(
        cancelled_before_handoff,
        "the acquisition future was cancelled"
    );
    assert!(gate_released, "the blocking worker's gate was released");
    assert!(
        no_retained_lease,
        "a cancelled, unhanded acquisition must release its lease"
    );
}

#[tokio::test]
async fn pi_receipt_selection_cannot_bypass_replaced_origin_fence() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, config_identity) = automated_input_fixture().await;
    config_identity
        .lock()
        .expect("fixture config")
        .resume_session = Some("thread-replaced".into());
    let automated = AutomatedTerminalInputSink::new(&state, "receiver", fence);
    let (receipt_tx, mut receipt_rx) = tokio::sync::mpsc::channel(1);
    let sender = select_delivery_input_sink(&automated, Some(&receipt_tx), true);

    let error = sender
        .send_bytes(b"reply for original conversation".to_vec())
        .await
        .expect_err("the automatic origin fence rejects the replaced conversation");

    assert!(error.retry_safe);
    assert!(
        input_rx.try_recv().is_err(),
        "the automated sink writes nothing"
    );
    assert!(
        receipt_rx.try_recv().is_err(),
        "the receipt sink cannot bypass it"
    );
}

#[tokio::test]
async fn unfenced_pi_ticket_selection_keeps_the_existing_receipt_write_path() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, _) = automated_input_fixture().await;
    let automated = AutomatedTerminalInputSink::new(&state, "receiver", fence);
    let (ticket_tx, mut ticket_rx) = tokio::sync::mpsc::channel(1);
    let sender = select_delivery_input_sink(&automated, Some(&ticket_tx), false);

    sender
        .send_bytes(b"manual Pi input".to_vec())
        .await
        .expect("unfenced delivery keeps the Pi ticket sink selected");

    assert_eq!(
        ticket_rx.recv().await.as_deref(),
        Some(b"manual Pi input".as_slice())
    );
    assert!(
        input_rx.try_recv().is_err(),
        "the alternate sink was selected"
    );
}

#[tokio::test]
async fn origin_change_after_payload_keeps_submit_uncertain_without_redirecting() {
    let _home = TestWardianHome::new_async().await;
    let (state, mut input_rx, fence, config_identity) = automated_input_fixture().await;
    let automated = AutomatedTerminalInputSink::new(&state, "receiver", fence);
    let (ticket_tx, mut ticket_rx) = tokio::sync::mpsc::channel(1);
    let sender = select_delivery_input_sink(&automated, Some(&ticket_tx), true);
    let mut profile = crate::utils::delivery_profile::delivery_profile("gemini");
    profile.submit_delay_ms = 0;
    let changed_config = config_identity.clone();

    let error = crate::utils::delivery_transaction::submit_terminal_transaction_with_hooks(
        sender,
        &profile,
        "reply payload",
        move || async move {
            changed_config
                .lock()
                .expect("fixture config")
                .resume_session = Some("thread-replaced-after-payload".into());
            Ok::<(), crate::utils::delivery_transaction::TerminalDeliveryError>(())
        },
        || async { Ok::<(), crate::utils::delivery_transaction::TerminalDeliveryError>(()) },
    )
    .await
    .expect_err("the changed origin blocks submit after payload delivery");

    assert!(
        !error.retry_safe,
        "sent payload makes the outcome uncertain"
    );
    assert_eq!(
        input_rx.recv().await.as_deref(),
        Some(b"reply payload".as_slice())
    );
    assert!(
        input_rx.try_recv().is_err(),
        "no submit key reaches the changed conversation"
    );
    assert!(
        ticket_rx.try_recv().is_err(),
        "the receipt sink receives no redirected submit"
    );
}

#[tokio::test]
async fn off_agent_keeps_new_wakeable_information_unclaimed() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender").await;
    agent(&state, "receiver").await;
    let config_handle = {
        let agents = state.agents.lock().await;
        agents.get("receiver").unwrap().config.clone()
    };
    {
        let mut config = config_handle.lock().unwrap();
        config.provider = "codex".into();
        config.resume_session = Some("thread-original".into());
    }
    state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Booting, None)
        .await;
    let origin = store::RecipientOrigin {
        provider: "codex".into(),
        conversation_id: "thread-original".into(),
        generation: 1,
    };
    let admitted = state
        .interactions
        .admit_agent_message_with_wake_on_admission(
            store::Admission {
                sender: "sender",
                recipient: "receiver",
                message: "FYI",
                idempotency_key: None,
                task: false,
                generation: 1,
            },
            Some(&origin),
        )
        .await
        .unwrap();

    dispatch_pending_wakeable_information(None, &state, "receiver")
        .await
        .unwrap();

    assert_eq!(
        store::with_db(|conn| {
            Ok(conn.query_row(
                "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
                [&admitted.record.id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap(),
        "stored"
    );
    assert_eq!(
        store::with_db(|conn| store::pending_wakeable_information(conn, "receiver")).unwrap(),
        vec![admitted.record.id]
    );
}

#[tokio::test]
async fn startup_does_not_wake_idle_agent_for_legacy_fyi() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender").await;
    agent(&state, "receiver").await;

    let (config_handle, status_handle) = {
        let agents = state.agents.lock().await;
        let receiver = agents.get("receiver").unwrap();
        (receiver.config.clone(), receiver.current_status.clone())
    };
    {
        let mut config = config_handle.lock().unwrap();
        config.provider = "codex".into();
        config.resume_session = Some("thread-original".into());
        config.is_off = false;
    }
    *status_handle.lock().unwrap() = "Idle".into();
    let generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let admitted = state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: "receiver",
            message: "legacy FYI",
            idempotency_key: None,
            task: false,
            generation,
        })
        .await
        .unwrap();

    super::super::dispatch_agent_messaging_from_status_observation(None, &state, "receiver").await;

    let owner = store::with_db(|conn| {
        Ok(conn.query_row(
            "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
            [&admitted.record.id],
            |row| row.get::<_, String>(0),
        )?)
    })
    .unwrap();
    assert_eq!(
        owner, "stored",
        "startup must not promote legacy FYIs to wake claims"
    );
    let highwater = store::with_db(|conn| store::information_highwater(conn, "receiver")).unwrap();
    assert_eq!(
        store::with_db(|conn| store::pending_information(conn, "receiver", highwater)).unwrap(),
        vec![admitted.record.id],
        "the legacy FYI remains available as bounded context"
    );
}

#[test]
fn wakeable_information_recovers_same_conversation_and_fences_replaced_session() {
    let origin = store::RecipientOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-original".into(),
        generation: 7,
    };
    assert_eq!(
        durable_conversation_matches(
            &origin.provider,
            &origin.conversation_id,
            "ANTIGRAVITY",
            Some("conversation-original"),
        ),
        DurableOriginMatch::Current,
        "a same-conversation restore may advance runtime generation"
    );
    assert_eq!(
        durable_conversation_matches(
            &origin.provider,
            &origin.conversation_id,
            "antigravity",
            Some("conversation-new"),
        ),
        DurableOriginMatch::Stale
    );
    assert_eq!(
        durable_conversation_matches(
            &origin.provider,
            &origin.conversation_id,
            "claude",
            Some("conversation-original"),
        ),
        DurableOriginMatch::Stale
    );
}

fn origin(id: &str) -> MessageOrigin {
    MessageOrigin::WardianAgent {
        session_id: id.into(),
    }
}

#[test]
fn surface_reply_fallback_requires_a_provider_turn_receipt() {
    assert!(!surface_has_dedicated_turn_receipt("antigravity"));
    assert!(surface_has_dedicated_turn_receipt("pi"));
    assert!(surface_has_dedicated_turn_receipt("opencode"));
}

#[tokio::test]
async fn retired_receive_and_wait_reject_without_claiming_or_acknowledging() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender").await;
    agent(&state, "receiver").await;
    let information = state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: "receiver",
            message: "FYI",
            idempotency_key: None,
            task: false,
            generation: 0,
        })
        .await
        .unwrap();
    let task = state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: "receiver",
            message: "work",
            idempotency_key: None,
            task: true,
            generation: 0,
        })
        .await
        .unwrap();

    for request in [
        Request::ReceiveMessages {
            cursor: None,
            ack_cursor: None,
            limit: None,
            timeout_ms: Some(0),
        },
        Request::WaitAgent {
            timeout_ms: Some(0),
        },
    ] {
        assert_eq!(
            handle_in_state(None, &state, request, origin("receiver"))
                .await
                .unwrap_err()
                .code,
            "automatic_delivery"
        );
    }

    let owner = |id: &str| {
        store::with_db(|conn| {
            Ok(conn.query_row(
                "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
                [id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap()
    };
    assert_eq!(owner(&information.record.id), "stored");
    assert_eq!(owner(&task.record.id), "pending");
    assert_eq!(
        store::with_db(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM agent_message_ack WHERE recipient='receiver'",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        state
            .agents
            .lock()
            .await
            .get("receiver")
            .unwrap()
            .current_status
            .lock()
            .unwrap()
            .as_str(),
        "Off"
    );
}

#[test]
fn reply_origin_recovers_across_runtime_generation_but_fences_new_provider_sessions() {
    let origin = store::RequesterOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-original".into(),
        generation: 7,
    };

    assert_eq!(
        durable_origin_matches(&origin, "Antigravity", Some("conversation-original"),),
        DurableOriginMatch::Current,
        "runtime restore may advance generation while preserving the provider conversation"
    );
    assert_eq!(
        durable_origin_matches(&origin, "antigravity", Some("conversation-new")),
        DurableOriginMatch::Stale
    );
    assert_eq!(
        durable_origin_matches(&origin, "claude", Some("conversation-original")),
        DurableOriginMatch::Stale
    );
    assert_eq!(
        durable_origin_matches(&origin, "antigravity", None),
        DurableOriginMatch::WaitingForIdentity
    );
}

#[tokio::test]
async fn explicit_duplicate_reply_wakeup_does_not_replay_a_delivered_continuation() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "requester").await;
    let requester_origin = store::RequesterOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-original".into(),
        generation: 3,
    };
    let task = store::with_db(|conn| {
        store::admit_with_requester_origin(
            conn,
            store::Admission {
                sender: "requester",
                recipient: "responder",
                message: "work",
                idempotency_key: None,
                task: true,
                generation: 1,
            },
            Some(&requester_origin),
        )
    })
    .unwrap();
    let reply = store::with_db(|conn| {
        store::reply(
            conn,
            "responder",
            &task.record.id,
            ReplyStatus::Done,
            "result",
        )
    })
    .unwrap();
    let claim = store::with_db(|conn| {
        store::claim_reply_continuation(conn, "requester", &reply.record.id, 4)
    })
    .unwrap()
    .unwrap();
    store::with_db(|conn| store::finish_claim(conn, &claim, "provider_visible")).unwrap();

    dispatch_pending_reply_continuations(None, &state, "requester", Some(&reply.record.id))
        .await
        .unwrap();

    assert_eq!(
        store::with_db(|conn| {
            Ok(conn.query_row(
                "SELECT continuation_state FROM agent_message_delivery
                 WHERE interaction_id=?1 AND operation='reply'",
                [&reply.record.id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap(),
        "delivered"
    );
    assert!(store::with_db(|conn| store::claim_reply_continuation(
        conn,
        "requester",
        &reply.record.id,
        5
    ))
    .unwrap()
    .is_none());
}
