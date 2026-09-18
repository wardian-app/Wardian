// Regression coverage for lifecycle contention at the live delivery boundary.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn write_pi_test_frame(stream: &mut TcpStream, value: &serde_json::Value) {
    let body = serde_json::to_vec(value).expect("encode Pi test frame");
    let length = u32::try_from(body.len()).expect("Pi test frame length");
    stream
        .write_all(&length.to_be_bytes())
        .await
        .expect("write Pi test frame length");
    stream
        .write_all(&body)
        .await
        .expect("write Pi test frame body");
}

async fn read_pi_test_frame(stream: &mut TcpStream) -> serde_json::Value {
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .expect("read Pi test frame length");
    let mut body = vec![0_u8; u32::from_be_bytes(prefix) as usize];
    stream
        .read_exact(&mut body)
        .await
        .expect("read Pi test frame body");
    serde_json::from_slice(&body).expect("decode Pi test frame")
}

#[tokio::test(flavor = "current_thread")]
async fn pi_capability_query_uses_ready_live_binding_for_current_generation() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    let target_agent_id = "pi-capability-agent";
    let generation = 7;
    state
        .interactions
        .record_provider_input_state(
            target_agent_id,
            generation,
            ProviderInputReadiness::Booting,
            None,
        )
        .await;

    let temp = tempfile::tempdir().expect("Pi capability fixture directory");
    let session_file = temp.path().join("session.jsonl");
    std::fs::write(&session_file, "{}\n").expect("write Pi session fixture");
    let extension_path = temp.path().join("extension.mjs");
    std::fs::write(&extension_path, "export default {};\n")
        .expect("write Pi extension fixture");
    let config = AgentConfig {
        provider: "pi".to_string(),
        session_id: target_agent_id.to_string(),
        folder: temp.path().display().to_string(),
        resume_session: Some("pi-session".to_string()),
        ..Default::default()
    };
    let plan = state
        .native_delivery
        .prepare_pi_tui(
            crate::delivery::native_broker::NativeSessionSpec {
                target_agent_id: target_agent_id.to_string(),
                provider: "pi".to_string(),
                generation,
                workspace: temp.path().to_path_buf(),
                config,
            },
            session_file.clone(),
            extension_path,
        )
        .await
        .expect("prepare Pi bridge");

    wardian_core::db::upsert_native_session_binding(
        &wardian_core::native_transport::NativeSessionBinding {
            target_agent_id: target_agent_id.to_string(),
            generation: generation - 1,
            provider: "pi".to_string(),
            transport: "stale".to_string(),
            provider_session_id: Some("stale-session".to_string()),
            capabilities: wardian_core::native_transport::NativeTransportCapabilities::degraded(
                "pi", "stale",
            ),
            observed_at: "2026-09-15T00:00:00Z".to_string(),
        },
    )
    .expect("write stale Pi binding fixture");
    assert!(native_capability_binding(&state, target_agent_id, "pi")
        .await
        .expect("query unready Pi capability")
        .is_none());

    let bridge_config: serde_json::Value =
        serde_json::from_str(plan.config()).expect("decode Pi bridge test config");
    plan.register_process(std::process::id());
    let host = bridge_config["host"].as_str().expect("Pi bridge host");
    let port = bridge_config["port"].as_u64().expect("Pi bridge port") as u16;
    let mut stream = TcpStream::connect((host, port))
        .await
        .expect("connect Pi bridge test client");
    let runtime_nonce = "pi-capability-test-nonce";
    write_pi_test_frame(
        &mut stream,
        &serde_json::json!({
            "version": 1,
            "target_id": target_agent_id,
            "generation": generation,
            "session_id": "pi-session",
            "runtime_nonce": runtime_nonce,
            "seq": 1,
            "type": "hello",
            "token": bridge_config["token"],
            "pid": std::process::id(),
            "session_file": session_file.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(read_pi_test_frame(&mut stream).await["type"], "welcome");
    write_pi_test_frame(
        &mut stream,
        &serde_json::json!({
            "version": 1,
            "target_id": target_agent_id,
            "generation": generation,
            "session_id": "pi-session",
            "runtime_nonce": runtime_nonce,
            "seq": 2,
            "type": "ready",
            "capabilities": {
                "task": true,
                "information": false,
                "cancel": false,
                "completion": false,
            },
        }),
    )
    .await;

    let binding = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Ok(Some(binding)) =
                native_capability_binding(&state, target_agent_id, "pi").await
            {
                break binding;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("Pi live capability should become ready");
    assert_eq!(binding.target_agent_id, target_agent_id);
    assert_eq!(binding.generation, generation);
    assert_eq!(binding.provider, "pi");
    assert_eq!(binding.transport, "pi_tui_bridge");
    assert_eq!(binding.provider_session_id.as_deref(), Some("pi-session"));
    assert!(binding.capabilities.persistent_session);
    assert!(native_capability_binding(&state, target_agent_id, "pi")
        .await
        .expect("query ready Pi capability")
        .is_some());
    assert!(state
        .native_delivery
        .pi_binding(target_agent_id, generation + 1)
        .await
        .is_err());

    state
        .native_delivery
        .dispose_pi_generation(target_agent_id, Some(generation))
        .await
        .expect("dispose Pi bridge");
    assert!(native_capability_binding(&state, target_agent_id, "pi")
        .await
        .expect("query disposed Pi capability")
        .is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn codex_capability_response_hides_unqualified_persisted_bindings() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    let cases = [
        ("none", None, "codex", false),
        ("blank", Some("   "), "codex", false),
        ("mismatch", Some("claude-session"), "claude", false),
        ("bound", Some("codex-session"), "codex", true),
    ];

    for (name, provider_session_id, stored_provider, expected_negotiated) in cases {
        let target_agent_id = format!("codex-capability-{name}");
        wardian_core::db::upsert_native_session_binding(
            &wardian_core::native_transport::NativeSessionBinding {
                target_agent_id: target_agent_id.clone(),
                generation: 1,
                provider: stored_provider.to_string(),
                transport: "codex_app_server_ws".to_string(),
                provider_session_id: provider_session_id.map(str::to_string),
                capabilities:
                    wardian_core::native_transport::NativeTransportCapabilities::degraded(
                        stored_provider,
                        "codex_app_server_ws",
                    ),
                observed_at: "2026-09-17T00:00:00Z".to_string(),
            },
        )
        .expect("write Codex capability fixture");

        let binding = native_capability_binding(&state, &target_agent_id, "codex")
            .await
            .expect("query Codex capability");
        let response = native_delivery_capabilities_response(
            target_agent_id,
            "codex",
            binding,
            wardian_core::native_transport::NativeTransportCapabilities::degraded(
                "codex",
                "unverified",
            ),
        );

        assert_eq!(
            response.native_negotiated, expected_negotiated,
            "unexpected negotiation state for {name} fixture"
        );
        assert_eq!(
            response.binding.is_some(),
            expected_negotiated,
            "unexpected binding presence for {name} fixture"
        );
    }
}

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
