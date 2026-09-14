//! Messaging authorization, ownership, correlation, and queue progression tests.
use super::super::test_support::TestWardianHome;
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use wardian_core::agent_messaging::{AgentMessagePage, TaskDeliveryOwner};
use wardian_core::control::ReplyStatus;

async fn agent(state: &AppState, id: &str, name: &str) {
    let agent = super::super::tests::test_agent(id, name, "Test");
    *agent.current_status.lock().unwrap() = "Off".into();
    state.agents.lock().await.insert(id.into(), agent);
}

fn origin(id: &str) -> MessageOrigin {
    MessageOrigin::WardianAgent {
        session_id: id.into(),
    }
}

async fn task(state: &AppState) -> store::Admitted {
    state
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
        .unwrap()
}

async fn receive(state: &AppState, recipient: &str) -> AgentMessagePage {
    let response = handle_in_state(
        None,
        state,
        Request::ReceiveMessages {
            cursor: None,
            ack_cursor: None,
            limit: None,
            timeout_ms: None,
        },
        origin(recipient),
    )
    .await
    .unwrap();
    let Response::ReceiveMessages { page } = response else {
        panic!("receive page")
    };
    page
}

fn owner(id: &str) -> String {
    store::with_db(|conn| {
        Ok(conn.query_row(
            "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
            [id],
            |row| row.get(0),
        )?)
    })
    .unwrap()
}

fn stored_status(id: &str) -> String {
    store::with_db(|conn| {
        Ok(
            conn.query_row("SELECT status FROM interactions WHERE id=?1", [id], |row| {
                row.get(0)
            })?,
        )
    })
    .unwrap()
}

fn opencode_json_response(
    status: axum::http::StatusCode,
    value: serde_json::Value,
) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(value.to_string()))
        .unwrap()
}

async fn opencode_test_server(
    request: axum::http::Request<axum::body::Body>,
    workspace: String,
    post_count: Option<Arc<AtomicUsize>>,
) -> axum::response::Response {
    use axum::http::{header::AUTHORIZATION, Method, StatusCode};

    if request.headers().get(AUTHORIZATION).is_none() {
        return opencode_json_response(StatusCode::UNAUTHORIZED, serde_json::json!({}));
    }

    let method = request.method().clone();
    let path = request.uri().path();
    match (method, path) {
        (Method::GET, "/global/health") => opencode_json_response(
            StatusCode::OK,
            serde_json::json!({
                "healthy": true,
                "version": "test",
            }),
        ),
        (Method::GET, "/session/ses_r13_test") => opencode_json_response(
            StatusCode::OK,
            serde_json::json!({
                "id": "ses_r13_test",
                "directory": workspace,
            }),
        ),
        (Method::GET, "/session/status") => opencode_json_response(
            StatusCode::OK,
            serde_json::json!({
                "ses_r13_test": "idle",
            }),
        ),
        (Method::GET, "/global/event") => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .body(axum::body::Body::from_stream(
                futures_util::stream::pending::<Result<String, std::convert::Infallible>>(),
            ))
            .unwrap(),
        (Method::POST, "/session/ses_r13_test/prompt_async") => {
            if let Some(post_count) = post_count {
                post_count.fetch_add(1, Ordering::SeqCst);
            }
            axum::response::Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(axum::body::Body::empty())
                .unwrap()
        }
        (Method::GET, "/session/ses_r13_test/message") => {
            opencode_json_response(StatusCode::OK, serde_json::json!([]))
        }
        _ => opencode_json_response(StatusCode::NOT_FOUND, serde_json::json!({})),
    }
}

#[tokio::test]
async fn attached_opencode_task_admits_native_record_before_followup() {
    use crate::delivery::native_broker::opencode_http_config_fingerprint;
    use crate::delivery::opencode_http::OpenCodeHttpLaunchPlan;
    use axum::{routing::any, Router};
    use wardian_core::control::ProviderInputReadiness;
    use wardian_core::native_transport::NativeDeliveryPhase;

    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    {
        let agents = state.agents.lock().await;
        let receiver = agents.get("receiver").unwrap();
        let mut config = receiver.config.lock().unwrap();
        config.provider = "opencode".into();
        config.folder = home.path().to_string_lossy().into_owned();
        config.resume_session = Some("ses_r13_test".into());
        *receiver.current_status.lock().unwrap() = "Idle".into();
    }
    let generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    assert_eq!(generation, 1);

    let info = delivery_target_info(&state, "receiver").await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let workspace = info.cwd.to_string_lossy().into_owned();
    let app = Router::new().fallback(any(move |request| {
        let workspace = workspace.clone();
        async move { opencode_test_server(request, workspace, None).await }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let fingerprint = opencode_http_config_fingerprint(&info.config, &info.cwd);
    state
        .native_delivery
        .prepare_opencode_http("receiver", generation, fingerprint.clone())
        .await
        .unwrap();
    state
        .native_delivery
        .register_opencode_http(
            "receiver".into(),
            OpenCodeHttpLaunchPlan::new(generation, 1, port).unwrap(),
            "ses_r13_test".into(),
            "test-process".into(),
            "test-listener".into(),
            info.cwd.clone(),
            fingerprint,
        )
        .await
        .unwrap();

    let admitted = task(&state).await;
    assert!(
        wardian_core::db::native_delivery(&admitted.record.id)
            .unwrap()
            .is_none(),
        "core admission must not pre-create the native delivery row"
    );
    let dispatch_result = dispatch_pending_queue(None, &state, "receiver").await;
    let native_dispose = state
        .native_delivery
        .dispose_opencode_http("receiver", Some(generation))
        .await;
    server.abort();
    native_dispose.unwrap();

    dispatch_result.unwrap();
    let native = wardian_core::db::native_delivery(&admitted.record.id)
        .unwrap()
        .expect("attached dispatch must admit a native delivery");
    assert_eq!(native.phase, NativeDeliveryPhase::ProviderAccepted);
    assert_eq!(owner(&admitted.record.id), "provider_accepted");
}

#[tokio::test]
async fn attached_opencode_terminal_native_phase_does_not_replay_or_release_task() {
    use crate::delivery::native_broker::{
        opencode_http_config_fingerprint, NativeDeliveryAdmission,
    };
    use crate::delivery::opencode_http::OpenCodeHttpLaunchPlan;
    use axum::{routing::any, Router};
    use wardian_core::control::ProviderInputReadiness;
    use wardian_core::native_transport::{NativeDeliveryPhase, NativeMessageOperation};

    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    {
        let agents = state.agents.lock().await;
        let receiver = agents.get("receiver").unwrap();
        let mut config = receiver.config.lock().unwrap();
        config.provider = "opencode".into();
        config.folder = home.path().to_string_lossy().into_owned();
        config.resume_session = Some("ses_r13_test".into());
        *receiver.current_status.lock().unwrap() = "Idle".into();
    }
    let generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    assert_eq!(generation, 1);

    let info = delivery_target_info(&state, "receiver").await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let workspace = info.cwd.to_string_lossy().into_owned();
    let post_count = Arc::new(AtomicUsize::new(0));
    let app = Router::new().fallback(any({
        let post_count = Arc::clone(&post_count);
        move |request| {
            let workspace = workspace.clone();
            let post_count = Arc::clone(&post_count);
            async move { opencode_test_server(request, workspace, Some(post_count)).await }
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let fingerprint = opencode_http_config_fingerprint(&info.config, &info.cwd);
    state
        .native_delivery
        .prepare_opencode_http("receiver", generation, fingerprint.clone())
        .await
        .unwrap();
    state
        .native_delivery
        .register_opencode_http(
            "receiver".into(),
            OpenCodeHttpLaunchPlan::new(generation, 1, port).unwrap(),
            "ses_r13_test".into(),
            "test-process".into(),
            "test-listener".into(),
            info.cwd.clone(),
            fingerprint,
        )
        .await
        .unwrap();

    let failed_task = task(&state).await;
    let uncertain_task = task(&state).await;
    let dispatching_task = task(&state).await;
    let native_admission = |admitted: &store::Admitted| NativeDeliveryAdmission {
        interaction_id: admitted.record.id.clone(),
        message_id: admitted.record.id.clone(),
        target_agent_id: "receiver".into(),
        sender_agent_id: admitted.record.sender_session_id.clone(),
        provider: "opencode".into(),
        generation,
        operation: NativeMessageOperation::StartTurn,
        caller_idempotency_key: Some(admitted.record.id.clone()),
        parent_interaction_id: admitted.record.parent_interaction_id.clone(),
        deadline_at: None,
        body: match &admitted.record.body_ref {
            wardian_core::control::InteractionBodyRef::Inline { body } => body.clone(),
            wardian_core::control::InteractionBodyRef::File { .. } => {
                unreachable!("test task bodies are inline")
            }
        },
    };
    let mut failed_record = state
        .native_delivery
        .admit(native_admission(&failed_task))
        .await
        .unwrap();
    failed_record.phase = NativeDeliveryPhase::FailedBeforeSubmit;
    failed_record.detail = Some("pre-provider submission failed".into());
    wardian_core::db::upsert_native_delivery(&failed_record).unwrap();

    let mut uncertain_record = state
        .native_delivery
        .admit(native_admission(&uncertain_task))
        .await
        .unwrap();
    uncertain_record.phase = NativeDeliveryPhase::SubmittedUnconfirmed;
    uncertain_record.detail = Some("provider boundary was crossed without confirmation".into());
    wardian_core::db::upsert_native_delivery(&uncertain_record).unwrap();

    let mut dispatching_record = state
        .native_delivery
        .admit(native_admission(&dispatching_task))
        .await
        .unwrap();
    dispatching_record.phase = NativeDeliveryPhase::Dispatching;
    dispatching_record.detail = Some("dispatch persistence is ambiguous".into());
    wardian_core::db::upsert_native_delivery(&dispatching_record).unwrap();

    let dispatch_result = dispatch_pending_queue(None, &state, "receiver").await;
    let native_dispose = state
        .native_delivery
        .dispose_opencode_http("receiver", Some(generation))
        .await;
    server.abort();

    dispatch_result.unwrap();
    native_dispose.unwrap();
    assert_eq!(owner(&failed_task.record.id), "failed_before_submit");
    assert_eq!(owner(&uncertain_task.record.id), "uncertain");
    assert_eq!(owner(&dispatching_task.record.id), "uncertain");
    assert_eq!(stored_status(&failed_task.record.id), "awaiting_reply");
    assert_eq!(stored_status(&uncertain_task.record.id), "awaiting_reply");
    assert_eq!(stored_status(&dispatching_task.record.id), "awaiting_reply");
    assert!(
        store::with_db(|conn| store::next_pending_task_id(conn, "receiver"))
            .unwrap()
            .is_none()
    );
    assert_eq!(post_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        wardian_core::db::native_delivery(&dispatching_task.record.id)
            .unwrap()
            .unwrap()
            .phase,
        NativeDeliveryPhase::Dispatching
    );

    dispatch_pending_queue(None, &state, "receiver")
        .await
        .unwrap();
    assert_eq!(post_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        wardian_core::db::native_delivery(&failed_task.record.id)
            .unwrap()
            .unwrap()
            .phase,
        NativeDeliveryPhase::FailedBeforeSubmit
    );
    assert_eq!(
        wardian_core::db::native_delivery(&uncertain_task.record.id)
            .unwrap()
            .unwrap()
            .phase,
        NativeDeliveryPhase::SubmittedUnconfirmed
    );
}

#[test]
fn prepared_pi_attached_task_selects_native_before_surface_fallback() {
    assert!(native_attached_owner_is_selected("pi", true));
    assert!(!native_attached_owner_is_selected("pi", false));
    assert!(!native_attached_owner_is_selected("opencode", true));
    assert!(!native_attached_owner_is_selected("claude", true));
}

#[test]
fn opencode_pending_or_failed_owner_has_zero_pty_writes_and_ready_uses_native() {
    use crate::delivery::native_broker::OpenCodeHttpEligibility;

    let eligible_states = [
        OpenCodeHttpEligibility::Pending,
        OpenCodeHttpEligibility::Failed,
        OpenCodeHttpEligibility::Ready,
    ];
    let mut composer_pty_writes = 0;
    for eligibility in eligible_states {
        assert_eq!(opencode_task_route(eligibility), OpenCodeTaskRoute::Native);
        if matches!(
            opencode_task_route(eligibility),
            OpenCodeTaskRoute::Composer
        ) {
            composer_pty_writes += 1;
        }
    }
    assert_eq!(composer_pty_writes, 0);
    assert_eq!(
        opencode_task_route(OpenCodeHttpEligibility::Unsupported),
        OpenCodeTaskRoute::Composer
    );
}

#[tokio::test(flavor = "current_thread")]
async fn opencode_owner_handoff_states_route_without_composer_pty_writes() {
    use crate::delivery::native_broker::{NativeDeliveryBroker, OpenCodeHttpEligibility};

    let broker = NativeDeliveryBroker::new();
    broker
        .prepare_opencode_http("opencode-agent", 7, "config-7".into())
        .await
        .expect("eligible owner state");
    let pending = broker.opencode_http_eligibility("opencode-agent", 7).await;
    assert_eq!(pending, OpenCodeHttpEligibility::Pending);
    assert_eq!(opencode_task_route(pending), OpenCodeTaskRoute::Native);

    broker.fail_opencode_http("opencode-agent", 7).await;
    let failed = broker.opencode_http_eligibility("opencode-agent", 7).await;
    assert_eq!(failed, OpenCodeHttpEligibility::Failed);
    assert_eq!(opencode_task_route(failed), OpenCodeTaskRoute::Native);
    assert_eq!(
        opencode_task_route(OpenCodeHttpEligibility::Ready),
        OpenCodeTaskRoute::Native
    );
}

fn count(table: &str) -> i64 {
    // Only fixed test-owned table names are passed by these tests.
    store::with_db(|conn| {
        Ok(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    })
    .unwrap()
}

#[tokio::test]
async fn managed_auth_and_exact_collision_fail_before_admission() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    agent(&state, "other", "receiver").await;
    let send = |target: &str| Request::SendMessage {
        target: target.into(),
        message: "hello".into(),
        idempotency_key: None,
    };
    for sender in ["", "missing"] {
        assert_eq!(
            handle_in_state(None, &state, send("Receiver"), origin(sender))
                .await
                .unwrap_err()
                .code,
            "unauthorized"
        );
    }
    assert_eq!(
        handle_in_state(None, &state, send("receiver"), origin("sender"))
            .await
            .unwrap_err()
            .code,
        "ambiguous_target"
    );
    for target in [
        "all",
        "ALL",
        "*",
        "broadcast",
        "class:Test",
        " Receiver",
        "Receiver ",
    ] {
        assert_eq!(
            handle_in_state(None, &state, send(target), origin("sender"))
                .await
                .unwrap_err()
                .code,
            "invalid_target"
        );
    }
    assert_eq!(
        handle_in_state(None, &state, send("Receiv"), origin("sender"))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    // Exact recovered edit 16337480: use unambiguous name here, so the test
    // reaches unsupported interrupt rather than the name/UUID collision above.
    assert_eq!(
        handle_in_state(
            None,
            &state,
            Request::InterruptAgent {
                target: "Receiver".into(),
            },
            origin("sender")
        )
        .await
        .unwrap_err()
        .code,
        "unsupported_interrupt"
    );
    assert_eq!(count("interactions"), 0);
    assert_eq!(count("agent_message_delivery"), 0);
    assert_eq!(count("agent_message_availability"), 0);
    let receipt = handle_in_state(None, &state, send("Receiver"), origin("sender"))
        .await
        .unwrap();
    assert!(
        matches!(receipt, Response::SendMessage { delivery_state, duplicate: false, .. } if delivery_state == "stored")
    );
    assert_eq!(
        receive(&state, "receiver").await.messages[0].message,
        "hello"
    );
}

#[tokio::test]
async fn off_info_stays_off_task_receipt_is_async_and_receiver_reply_is_correlated() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let literal = "  中文😀\nquoted \"text\"\\path\r\n ";
    let sent = handle_in_state(
        None,
        &state,
        Request::SendMessage {
            target: "Receiver".into(),
            message: literal.into(),
            idempotency_key: None,
        },
        origin("sender"),
    )
    .await
    .unwrap();
    let Response::SendMessage {
        interaction_id,
        delivery_state,
        duplicate,
    } = sent
    else {
        panic!("message receipt")
    };
    assert_eq!(delivery_state, "stored");
    assert!(!duplicate);
    let canonical = state
        .interactions
        .interaction(&interaction_id)
        .await
        .unwrap();
    assert_eq!(
        canonical.kind,
        wardian_core::control::InteractionKind::Message
    );
    assert_eq!(
        canonical.trigger_policy,
        wardian_core::control::InteractionTriggerPolicy::NotifyOnly
    );
    assert_eq!(
        canonical.body_ref,
        wardian_core::control::InteractionBodyRef::Inline {
            body: literal.into()
        }
    );
    assert_eq!(
        delivery_target_info(&state, "receiver")
            .await
            .unwrap()
            .status,
        "off"
    );
    assert!(
        store::with_db(|conn| store::claim_next_task(conn, "receiver", 0))
            .unwrap()
            .is_none()
    );
    // An admission must return even while the receiver's lifecycle is owned.
    // Awaiting execution here would block on this guard and fail the timeout.
    let lifecycle = state.lock_agent_lifecycle("receiver").await;
    let task = tokio::time::timeout(
        Duration::from_secs(1),
        handle_in_state(
            None,
            &state,
            Request::FollowupTask {
                target: "Receiver".into(),
                message: "work".into(),
                idempotency_key: None,
            },
            origin("sender"),
        ),
    )
    .await
    .expect("admission must not await execution")
    .unwrap();
    drop(lifecycle);
    let Response::FollowupTask {
        request_id,
        delivery_state,
        delivery_owner,
        ..
    } = task
    else {
        panic!("task receipt")
    };
    assert_eq!(delivery_state, "pending");
    assert_eq!(delivery_owner, TaskDeliveryOwner::Unclaimed);
    assert_eq!(stored_status(&request_id), "awaiting_reply");
    let received = receive(&state, "receiver").await;
    assert_eq!(received.messages.len(), 2);
    assert_eq!(received.messages[0].interaction_id, interaction_id);
    assert_eq!(received.messages[0].message.as_bytes(), literal.as_bytes());
    assert_eq!(received.messages[1].interaction_id, request_id);
    assert_eq!(owner(&request_id), "receiver_available");
    assert!(
        store::with_db(|conn| store::claim_next_task(conn, "receiver", 0))
            .unwrap()
            .is_none()
    );
    assert_eq!(receive(&state, "receiver").await, received);
    let reply_request = || Request::Reply {
        request_id: request_id.clone(),
        status: ReplyStatus::Done,
        message: "done\nλ".into(),
    };
    assert_eq!(
        handle_in_state(None, &state, reply_request(), origin("sender"))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(stored_status(&request_id), "awaiting_reply");
    let response = handle_in_state(None, &state, reply_request(), origin("receiver"))
        .await
        .unwrap();
    let Response::Reply {
        interaction_id: reply_id,
        duplicate,
        ..
    } = response
    else {
        panic!("reply receipt")
    };
    assert!(!duplicate);
    let reply_page = receive(&state, "sender").await;
    assert_eq!(reply_page.messages.len(), 1);
    let reply = &reply_page.messages[0];
    assert_eq!(reply.interaction_id, reply_id);
    assert_eq!(reply.kind, wardian_core::control::InteractionKind::Reply);
    assert_eq!(reply.sender, "receiver");
    assert_eq!(
        reply.parent_interaction_id.as_deref(),
        Some(request_id.as_str())
    );
    assert_eq!(reply.reply_status, Some(ReplyStatus::Done));
    assert_eq!(reply.message, "done\nλ");
    assert_eq!(stored_status(&request_id), "completed");
    assert_eq!(
        state
            .interactions
            .structured_reply(&request_id)
            .await
            .unwrap()
            .body,
        "done\nλ"
    );
    let repeated = handle_in_state(None, &state, reply_request(), origin("receiver"))
        .await
        .unwrap();
    assert!(
        matches!(repeated, Response::Reply { interaction_id, duplicate: true, .. } if interaction_id == reply_id)
    );
    assert_eq!(count("structured_replies"), 1);
    assert_eq!(receive(&state, "sender").await, reply_page);
    assert_eq!(
        delivery_target_info(&state, "receiver")
            .await
            .unwrap()
            .status,
        "off"
    );
}

#[tokio::test]
async fn receive_timeout_does_not_complete_or_cancel_task() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    let response = handle_in_state(
        None,
        &state,
        Request::ReceiveMessages {
            cursor: None,
            ack_cursor: None,
            limit: None,
            timeout_ms: Some(5),
        },
        origin("sender"),
    )
    .await
    .unwrap();
    assert!(
        matches!(response, Response::ReceiveMessages { page } if page.timed_out && page.messages.is_empty() && page.wake_reason.is_none())
    );
    assert_eq!(stored_status(&task.record.id), "awaiting_reply");
    assert_eq!(owner(&task.record.id), "pending");
    assert!(state
        .interactions
        .structured_reply(&task.record.id)
        .await
        .is_none());
    let late = state
        .interactions
        .reply_agent_message(
            "receiver",
            &task.record.id,
            ReplyStatus::Done,
            "late result",
        )
        .await
        .unwrap();
    assert_eq!(
        receive(&state, "sender").await.messages[0].interaction_id,
        late.record.id
    );
    assert_eq!(
        receive(&state, "sender").await.messages[0].message,
        "late result"
    );
}

#[tokio::test]
async fn replacement_rejects_stale_claim_and_late_receipt_without_replay() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await;
    assert_eq!(
        state
            .interactions
            .claim_agent_task("receiver", 0)
            .await
            .err()
            .unwrap()
            .code,
        "stale_claim"
    );
    let claim = state
        .interactions
        .claim_agent_task("receiver", 1)
        .await
        .unwrap()
        .unwrap();
    // Exact recovered validation assertions from source edit 16363204.
    state
        .interactions
        .validate_agent_message_claim(&claim)
        .await
        .unwrap();
    state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Booting, None)
        .await;
    assert_eq!(
        state
            .interactions
            .validate_agent_message_claim(&claim)
            .await
            .unwrap_err()
            .code,
        "stale_claim"
    );
    assert_eq!(
        state
            .interactions
            .finish_agent_task(&claim, "provider_accepted")
            .await
            .unwrap_err()
            .code,
        "stale_claim"
    );
    assert_eq!(owner(&task.record.id), "uncertain");
    assert_eq!(stored_status(&task.record.id), "awaiting_reply");
    assert_eq!(
        state
            .interactions
            .agent_message_provider_revision("receiver")
            .await,
        0
    );
    assert!(state
        .interactions
        .claim_agent_task("receiver", 2)
        .await
        .unwrap()
        .is_none());
    assert!(receive(&state, "receiver").await.messages.is_empty());
    assert!(state
        .interactions
        .release_agent_message_before_write(&claim)
        .await
        .is_err());
    assert_eq!(owner(&task.record.id), "uncertain");
    assert_eq!(
        state
            .interactions
            .fail_agent_startup(&claim)
            .await
            .err()
            .expect("uncertainty must not become a startup failure reply")
            .code,
        "stale_claim"
    );
    assert_eq!(owner(&task.record.id), "uncertain");
    assert_eq!(stored_status(&task.record.id), "awaiting_reply");
    assert_eq!(count("structured_replies"), 0);
    assert!(state
        .interactions
        .structured_reply(&task.record.id)
        .await
        .is_none());
    assert!(receive(&state, "sender").await.messages.is_empty());
}

#[tokio::test]
async fn startup_failure_after_generation_replacement_settles_exact_claim_without_replay() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    let claim = state
        .interactions
        .claim_agent_task("receiver", 0)
        .await
        .unwrap()
        .unwrap();
    // Exercise settlement after replacement, as can follow lease expiry. This
    // direct state transition does not model bypassing an active lifecycle lease.
    // No provider submission has occurred for this captured claim.
    let replacement = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Booting, None)
        .await;
    assert_eq!(
        state
            .interactions
            .validate_agent_message_claim(&claim)
            .await
            .unwrap_err()
            .code,
        "stale_claim"
    );
    let replied = state.interactions.fail_agent_startup(&claim).await.unwrap();
    assert_eq!(owner(&task.record.id), "failed_before_submit");
    assert_eq!(stored_status(&task.record.id), "completed");
    assert_eq!(replied.reply.status, ReplyStatus::Failed);
    assert_eq!(replied.reply.request_id, task.record.id);
    assert_eq!(
        replied.record.parent_interaction_id.as_deref(),
        Some(task.record.id.as_str())
    );
    assert_eq!(
        state
            .interactions
            .interaction(&task.record.id)
            .await
            .unwrap(),
        replied.task
    );
    assert_eq!(
        state
            .interactions
            .structured_reply(&task.record.id)
            .await
            .unwrap(),
        replied.reply
    );
    assert_eq!(owner(&replied.record.id), "stored");
    let page = receive(&state, "sender").await;
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].interaction_id, replied.record.id);
    assert_eq!(page.messages[0].reply_status, Some(ReplyStatus::Failed));
    assert_eq!(
        page.messages[0].parent_interaction_id.as_deref(),
        Some(task.record.id.as_str())
    );
    assert_eq!(page.messages[0].message, replied.reply.body);
    assert!(page.wake_reason.is_none());
    assert!(receive(&state, "receiver").await.messages.is_empty());
    assert!(state
        .interactions
        .claim_agent_task("receiver", replacement.generation)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        state
            .interactions
            .fail_agent_startup(&claim)
            .await
            .err()
            .expect("a settled claim cannot publish another reply")
            .code,
        "stale_claim"
    );
    assert_eq!(count("structured_replies"), 1);
    assert_eq!(receive(&state, "sender").await, page);
    assert_eq!(
        state.interactions.provider_input_state("receiver").await,
        Some(replacement)
    );
    for recipient in ["sender", "receiver"] {
        assert_eq!(
            state
                .interactions
                .agent_message_provider_revision(recipient)
                .await,
            0
        );
        assert_eq!(
            delivery_target_info(&state, recipient)
                .await
                .unwrap()
                .status,
            "off"
        );
    }
}

#[tokio::test]
async fn startup_failure_rejects_reclaimed_token_without_changing_new_claim() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    let old = state
        .interactions
        .claim_agent_task("receiver", 0)
        .await
        .unwrap()
        .unwrap();
    state
        .interactions
        .release_agent_message_before_write(&old)
        .await
        .unwrap();
    let generation = state
        .interactions
        .start_provider_input_generation("receiver", ProviderInputReadiness::Booting, None)
        .await
        .generation;
    let current = state
        .interactions
        .claim_agent_task("receiver", generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.record.id, old.record.id);
    assert_ne!(current.token, old.token);
    assert_ne!(current.generation, old.generation);
    assert_eq!(
        state
            .interactions
            .fail_agent_startup(&old)
            .await
            .err()
            .expect("old token must not settle a reclaimed task")
            .code,
        "stale_claim"
    );
    // Independently retain both persisted fences: a matching generation cannot
    // redeem the old token, and a matching token cannot redeem the old generation.
    for mismatched in [
        store::TaskClaim {
            record: current.record.clone(),
            token: old.token.clone(),
            generation: current.generation,
        },
        store::TaskClaim {
            record: current.record.clone(),
            token: current.token.clone(),
            generation: old.generation,
        },
    ] {
        assert_eq!(
            state
                .interactions
                .fail_agent_startup(&mismatched)
                .await
                .err()
                .expect("both stored token and generation must match")
                .code,
            "stale_claim"
        );
    }
    assert!(store::with_db(|conn| store::owns_claim(conn, &current)).unwrap());
    state
        .interactions
        .validate_agent_message_claim(&current)
        .await
        .unwrap();
    assert_eq!(owner(&task.record.id), "dispatching");
    assert_eq!(stored_status(&task.record.id), "awaiting_reply");
    assert_eq!(
        state
            .interactions
            .interaction(&task.record.id)
            .await
            .unwrap(),
        task.record
    );
    assert_eq!(count("interactions"), 1);
    assert_eq!(count("agent_message_delivery"), 1);
    assert_eq!(count("structured_replies"), 0);
    assert!(receive(&state, "sender").await.messages.is_empty());
}

#[tokio::test]
async fn startup_failure_after_deletion_does_not_recreate_claim_or_reply() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    let claim = state
        .interactions
        .claim_agent_task("receiver", 0)
        .await
        .unwrap()
        .unwrap();
    state
        .interactions
        .delete_agent_durable_state("receiver")
        .await
        .unwrap();
    assert_eq!(
        state
            .interactions
            .fail_agent_startup(&claim)
            .await
            .err()
            .expect("deleted recipient must reject late failure publication")
            .code,
        "unauthorized"
    );
    assert!(state
        .interactions
        .interaction(&task.record.id)
        .await
        .is_none());
    assert!(state
        .interactions
        .structured_reply(&task.record.id)
        .await
        .is_none());
    for table in [
        "interactions",
        "agent_message_delivery",
        "agent_message_availability",
        "structured_replies",
    ] {
        assert_eq!(count(table), 0, "late failure recreated {table}");
    }
}

#[tokio::test]
async fn discovery_uses_normal_neighbors_including_self_and_excludes_unrelated_roster() {
    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    agent(&state, "unrelated", "Unrelated").await;
    let mut topology = wardian_core::topology::Topology::default();
    assert!(topology.add_edge("sender", "receiver", "2026-09-07T00:00:00Z"));
    wardian_core::topology::save_topology(home.path(), &topology).unwrap();
    let response = handle_in_state(None, &state, Request::ListAgents, origin("sender"))
        .await
        .unwrap();
    let Response::ListAgents { agents } = response else {
        panic!("agents")
    };
    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.uuid.as_str())
            .collect::<Vec<_>>(),
        ["receiver", "sender"]
    );
    assert_eq!(agents[0].visibility.as_deref(), Some("manual"));
    assert_eq!(agents[1].visibility, None);
    // Discovery is narrower than the still-supported explicit exact targeting.
    assert_eq!(
        resolve_exact(&state, "Unrelated").await.unwrap(),
        "unrelated"
    );
    assert_eq!(count("interactions"), 0);
}

#[tokio::test]
async fn deleted_receiver_cannot_recreate_cursor_metadata_after_authentication() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    authenticate(&state, "receiver").await.unwrap();
    let page = receive(&state, "receiver").await;
    assert_eq!(page.messages[0].interaction_id, task.record.id);
    assert!(page.messages[0].host_automation.is_none());
    assert!(count("agent_message_cursors") > 0);
    // Model an already-authenticated call retaining a roster entry while durable
    // deletion commits. InteractionState's own fence must reject the late read.
    state
        .interactions
        .delete_agent_durable_state("receiver")
        .await
        .unwrap();
    authenticate(&state, "receiver").await.unwrap();
    assert_eq!(
        state
            .interactions
            .receive_agent_messages(
                "receiver",
                Some(&page.next_cursor),
                Some(&page.ack_cursor),
                100
            )
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(
        handle_in_state(
            None,
            &state,
            Request::ReceiveMessages {
                cursor: None,
                ack_cursor: None,
                limit: None,
                timeout_ms: None
            },
            origin("receiver")
        )
        .await
        .unwrap_err()
        .code,
        "unauthorized"
    );
    assert!(state
        .interactions
        .interaction(&task.record.id)
        .await
        .is_none());
    for table in [
        "interactions",
        "agent_message_delivery",
        "agent_message_availability",
        "agent_message_cursors",
        "agent_message_ack",
    ] {
        assert_eq!(count(table), 0, "deleted state recreated in {table}");
    }
    assert!(state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: "receiver",
            message: "late",
            idempotency_key: None,
            task: false,
            generation: 0,
        })
        .await
        .is_err());
    assert_eq!(count("interactions"), 0);
}

struct MockScriptGuard {
    previous: Option<std::ffi::OsString>,
    release: std::path::PathBuf,
}

impl Drop for MockScriptGuard {
    fn drop(&mut self) {
        // Release this test's child barrier even if an assertion panics.
        let _ = std::fs::write(&self.release, "continue");
        match self.previous.take() {
            Some(previous) => std::env::set_var("WARDIAN_MOCK_SCRIPT", previous),
            None => std::env::remove_var("WARDIAN_MOCK_SCRIPT"),
        }
    }
}

#[tokio::test]
async fn concurrent_off_tasks_progress_after_lease_release_without_idle_event() {
    let test_home = TestWardianHome::new_async().await;
    let script = test_home.path().join("queue-child.cjs");
    // Exact recovered real-child program and queue/lease assertions: 16345476.
    std::fs::write(&script, "const fs=require('node:fs'); const path=require('node:path'); fs.appendFileSync(path.join(__dirname,'child-count.txt'),'started\\n'); const timer=setInterval(()=>{if(fs.existsSync(path.join(__dirname,'release'))){clearInterval(timer);console.log(JSON.stringify({response:'completed'}));}},10);").unwrap();
    let _script_guard = MockScriptGuard {
        previous: std::env::var_os("WARDIAN_MOCK_SCRIPT"),
        release: test_home.path().join("release"),
    };
    std::env::set_var("WARDIAN_MOCK_SCRIPT", &script);
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    {
        let agents = state.agents.lock().await;
        let recipient = agents.get("receiver").unwrap();
        let mut config = recipient.config.lock().unwrap();
        config.provider = "mock".into();
        config.folder = test_home.path().to_string_lossy().into_owned();
    }
    let first = task(&state).await;
    let info = delivery_target_info(&state, "receiver").await.unwrap();
    let queue = dispatch_pending_queue(None, &state, "receiver");
    let concurrent = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !test_home.path().join("child-count.txt").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first child must reach barrier");
        assert!(active_conversation_lease_for_delivery(&info));
        assert_eq!(owner(&first.record.id), "dispatching");
        state
            .interactions
            .admit_agent_message(store::Admission {
                sender: "sender",
                recipient: "receiver",
                message: "second",
                idempotency_key: None,
                task: true,
                generation: 0,
            })
            .await
            .unwrap();
        // This is the same opportunity new admissions spawn in production. The
        // active acquisition prevents it from starting a competing child.
        dispatch_pending_queue(None, &state, "receiver")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(test_home.path().join("child-count.txt")).unwrap(),
            "started\n"
        );
        std::fs::write(test_home.path().join("release"), "continue").unwrap();
    };
    let (result, ()) = tokio::join!(queue, concurrent);
    result.unwrap();
    assert_eq!(
        std::fs::read_to_string(test_home.path().join("child-count.txt")).unwrap(),
        "started\nstarted\n"
    );
    assert!(!active_conversation_lease_for_delivery(&info));
    assert!(
        store::with_db(|conn| store::next_pending_task_id(conn, "receiver"))
            .unwrap()
            .is_none()
    );
    let rows: Vec<(String, String)> = store::with_db(|conn| {
        let mut statement = conn.prepare("SELECT d.owner,i.status FROM agent_message_delivery d JOIN interactions i ON i.id=d.interaction_id WHERE d.operation='followup_task'")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|(owner, status)| owner == "provider_visible" && status == "awaiting_reply"));
    assert!(receive(&state, "receiver").await.messages.is_empty());
    // Repeated opportunities cannot replay either already-claimed task.
    dispatch_pending_queue(None, &state, "receiver")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(test_home.path().join("child-count.txt")).unwrap(),
        "started\nstarted\n"
    );
}

#[tokio::test]
async fn off_codex_status_drift_dispatches_second_task_after_lease_without_replay() {
    let fixture = TestWardianHome::new_async().await;
    let state = AppState::new();
    const RECEIVER: &str = "11111111-1111-4111-8111-111111111218";
    const KEY: &str = "WARDIAN_CODEX_STARTUP_TEST_TOKEN";
    // The real owner's first guard rejects this synthetic value before habitat
    // preparation or process launch. TestWardianHome serializes environment use.
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var(KEY, previous),
                None => std::env::remove_var(KEY),
            }
        }
    }
    let _restore = Restore(std::env::var_os(KEY));
    std::env::set_var(KEY, RECEIVER);
    agent(&state, "sender", "Sender").await;
    let mut recipient = super::super::tests::test_agent(RECEIVER, "Receiver", "Test");
    recipient.process_id = None;
    {
        let mut config = recipient.config.lock().unwrap();
        config.provider = "codex".into();
        config.is_off = true;
        config.folder = fixture.path().to_string_lossy().into_owned();
        config.resume_session = Some("22222222-2222-4222-8222-222222221218".into());
        config.session_persistence = wardian_core::models::AgentSessionPersistenceOverride::Resume;
    }
    let config = recipient.config.clone();
    let status = recipient.current_status.clone();
    *status.lock().unwrap() = "Off".into();
    state.agents.lock().await.insert(RECEIVER.into(), recipient);
    let info = delivery_target_info(&state, RECEIVER).await.unwrap();
    let execution =
        wardian_core::automation_execution_lock::acquire_headless_execution_guard().unwrap();
    let lease = acquire_headless_message_lease(&info, "first-background-owner").unwrap();
    let mut lease = wardian_core::conversation_lease::PersistedConversationLeaseGuard::new(&lease);
    let first_generation = state
        .interactions
        .start_provider_input_generation(RECEIVER, ProviderInputReadiness::Booting, None)
        .await
        .generation;
    let first = state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: RECEIVER,
            message: "first task",
            idempotency_key: None,
            task: true,
            generation: first_generation,
        })
        .await
        .unwrap();
    let first_claim = state
        .interactions
        .claim_agent_task(RECEIVER, first_generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_claim.record.id, first.record.id);
    // Synthesize the first provider's completed task, retaining its acquisition
    // across a later admission as real owner-exit cleanup does. No provider runs.
    state
        .interactions
        .reply_agent_message(
            RECEIVER,
            &first.record.id,
            ReplyStatus::Done,
            "first answer",
        )
        .await
        .unwrap();
    state
        .interactions
        .finish_agent_task(&first_claim, "provider_completed")
        .await
        .unwrap();
    let second = state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: RECEIVER,
            message: "second distinct task",
            idempotency_key: None,
            task: true,
            generation: first_generation,
        })
        .await
        .unwrap();
    assert_ne!(first.record.id, second.record.id);
    let snapshot = || {
        store::with_db(|conn| {
            let mut statement = conn.prepare(concat!(
                "SELECT interaction_id,owner,generation,claim_token ",
                "FROM agent_message_delivery WHERE operation='followup_task' ",
                "ORDER BY interaction_id",
            ))?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap()
    };
    let before = snapshot();
    assert_eq!(owner(&first.record.id), "provider_completed");
    assert_eq!(owner(&second.record.id), "pending");
    for drift in ["Processing...", "Idle"] {
        *status.lock().unwrap() = drift.into();
        let current = delivery_target_info(&state, RECEIVER).await.unwrap();
        assert_eq!(current.status, "off");
        assert!(active_conversation_lease_for_delivery(&current));
        dispatch_pending_queue(None, &state, RECEIVER)
            .await
            .unwrap();
        assert_eq!(
            snapshot(),
            before,
            "lease must prevent every claim: {drift}"
        );
        assert_eq!(
            state
                .interactions
                .current_provider_input_generation(RECEIVER)
                .await,
            Some(first_generation)
        );
    }
    lease.release().unwrap();
    drop(execution);
    assert!(!active_conversation_lease_for_delivery(&info));
    // Inject a late shared-status write after cleanup without claiming its writer.
    *status.lock().unwrap() = "Idle".into();

    // Configured-live Idle stays attached. With no attached owner it leaves work
    // unclaimed instead of starting a background generation.
    config.lock().unwrap().is_off = false;
    assert_eq!(
        delivery_target_info(&state, RECEIVER).await.unwrap().status,
        "idle"
    );
    dispatch_one(None, &state, RECEIVER).await.unwrap();
    assert_eq!(snapshot(), before);
    assert_eq!(
        state
            .interactions
            .current_provider_input_generation(RECEIVER)
            .await,
        Some(first_generation)
    );
    config.lock().unwrap().is_off = true;

    let error = tokio::time::timeout(Duration::from_secs(5), dispatch_one(None, &state, RECEIVER))
        .await
        .expect("process-free startup guard must finish promptly")
        .expect_err("configured Off must reach the real background startup guard");
    assert_eq!(error.code, "native_followup_unavailable");
    assert!(error
        .message
        .contains("session identifier matches a credential environment value"));
    let next_generation = state
        .interactions
        .current_provider_input_generation(RECEIVER)
        .await
        .unwrap();
    assert_eq!(next_generation, first_generation + 1);
    let after = snapshot();
    let first_before = before.iter().find(|row| row.0 == first.record.id).unwrap();
    assert_eq!(
        after.iter().find(|row| row.0 == first.record.id).unwrap(),
        first_before
    );
    let second_after = after.iter().find(|row| row.0 == second.record.id).unwrap();
    assert_eq!(second_after.1, "failed_before_submit");
    assert_eq!(second_after.2, next_generation);
    assert!(second_after.3.is_some());
    assert_eq!(stored_status(&first.record.id), "completed");
    assert_eq!(stored_status(&second.record.id), "completed");
    assert_eq!(
        state
            .interactions
            .structured_reply(&first.record.id)
            .await
            .unwrap()
            .body,
        "first answer"
    );
    assert_eq!(
        state
            .interactions
            .structured_reply(&second.record.id)
            .await
            .unwrap()
            .status,
        ReplyStatus::Failed
    );
    assert_eq!(count("structured_replies"), 2);
    assert!(wardian_core::conversation_lease::load_leases().is_empty());
    assert!(state
        .native_delivery
        .codex_binding(RECEIVER, next_generation)
        .await
        .is_err());
    let mutation = wardian_core::automation_execution_lock::try_acquire_worktree_mutation_guard()
        .unwrap()
        .expect("background execution guard must be released");
    drop(mutation);
    for _ in 0..2 {
        dispatch_pending_queue(None, &state, RECEIVER)
            .await
            .unwrap();
        assert_eq!(snapshot(), after, "neither task may be replayed");
        assert_eq!(
            state
                .interactions
                .current_provider_input_generation(RECEIVER)
                .await,
            Some(next_generation)
        );
        assert_eq!(count("structured_replies"), 2);
    }
}

#[tokio::test]
async fn context_failure_releases_claim_before_any_provider_boundary() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
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
    let replied = state
        .interactions
        .reply_agent_message("receiver", &task.record.id, ReplyStatus::Done, "done")
        .await
        .unwrap();
    let claim = state
        .interactions
        .claim_agent_information("sender", &replied.record.id, 0)
        .await
        .unwrap()
        .unwrap();
    store::with_db(|conn| {
        conn.execute(
            "DELETE FROM structured_replies WHERE request_id=?1",
            [&task.record.id],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(prepare_claim_context(&state, &claim).await.is_err());
    let owner: String = store::with_db(|conn| {
        Ok(conn.query_row(
            "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
            [&claim.record.id],
            |row| row.get(0),
        )?)
    })
    .unwrap();
    assert_eq!(owner, "stored");
    assert!(!store::with_db(|conn| store::owns_claim(conn, &claim)).unwrap());
}

#[tokio::test]
async fn retired_records_survive_restore_and_idle_without_pty_or_v2_replay() {
    use wardian_core::control::{MailboxDeliveryPhase, MailboxMessageRecord, MailboxMessageStatus};
    let _home = TestWardianHome::new_async().await;
    let original = AppState::new();
    for (index, (status, phase)) in [
        (MailboxMessageStatus::Pending, MailboxDeliveryPhase::Queued),
        (
            MailboxMessageStatus::InFlight,
            MailboxDeliveryPhase::Dispatching,
        ),
        (
            MailboxMessageStatus::InFlight,
            MailboxDeliveryPhase::Submitted,
        ),
        (
            MailboxMessageStatus::Delivered,
            MailboxDeliveryPhase::Terminal,
        ),
        (MailboxMessageStatus::Failed, MailboxDeliveryPhase::Terminal),
    ]
    .into_iter()
    .enumerate()
    {
        let message = original
            .interactions
            .create_message_durable(
                Some("old-sender".into()),
                vec!["receiver".into()],
                InteractionBodyRef::Inline {
                    body: format!("old payload {index}"),
                },
            )
            .await
            .unwrap();
        wardian_core::db::upsert_mailbox_message(&MailboxMessageRecord {
            id: format!("legacy-{index}"),
            interaction_id: message.id,
            target_session_id: "receiver".into(),
            body: format!("old payload {index}"),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: Some(origin("old-sender")),
            created_at: "2020-01-01T00:00:00Z".into(),
            status,
            phase,
        })
        .unwrap();
    }
    original
        .interactions
        .create_task(
            None,
            "receiver".into(),
            InteractionBodyRef::Inline {
                body: "old automation".into(),
            },
        )
        .await;
    let rows = wardian_core::db::list_mailbox_messages().unwrap();
    let interactions = wardian_core::db::list_interaction_records().unwrap();
    for _ in 0..2 {
        let restored = AppState::new();
        agent(&restored, "receiver", "Receiver").await;
        {
            let agents = restored.agents.lock().await;
            let recipient = agents.get("receiver").unwrap();
            recipient.config.lock().unwrap().provider = "codex".into();
            *recipient.current_status.lock().unwrap() = "Idle".into();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        restored
            .terminal_sessions
            .start_or_replace_runtime(
                "receiver",
                crate::state::terminal_session::TerminalRuntimeHandles::new(tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .unwrap();
        restored.interactions.hydrate_from_persistence().await;
        super::super::dispatch_agent_messaging_from_status_observation(None, &restored, "receiver")
            .await;
        assert!(
            rx.try_recv().is_err(),
            "legacy restore/idle must never write PTY"
        );
        assert!(receive(&restored, "receiver").await.messages.is_empty());
        assert!(
            store::with_db(|conn| store::next_pending_task_id(conn, "receiver"))
                .unwrap()
                .is_none()
        );
        assert_eq!(wardian_core::db::list_mailbox_messages().unwrap(), rows);
        assert_eq!(
            wardian_core::db::list_interaction_records().unwrap(),
            interactions
        );
        for record in &interactions {
            assert_eq!(
                restored.interactions.interaction(&record.id).await.as_ref(),
                Some(record)
            );
        }
    }
}

#[tokio::test]
async fn host_automation_uses_canonical_manual_claim_and_authorized_correlated_reply() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "receiver", "Receiver").await;
    agent(&state, "other", "Other").await;
    let task = state
        .interactions
        .admit_host_automation_task("run-1288", "review", "receiver", "host work")
        .await
        .unwrap();
    assert_eq!(task.record.sender_session_id, None);
    assert_eq!(
        store::with_db(|conn| store::host_automation_provenance(conn, &task.record.id)).unwrap(),
        Some(wardian_core::agent_messaging::HostAutomationProvenance {
            run_id: "run-1288".into(),
            node: "review".into()
        })
    );
    assert_eq!(
        state.agents.lock().await.len(),
        2,
        "host is not a fake agent"
    );
    let page = receive(&state, "receiver").await;
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].interaction_id, task.record.id);
    assert_eq!(
        page.messages[0].host_automation.as_ref().unwrap().run_id,
        "run-1288"
    );
    assert!(
        state
            .interactions
            .claim_agent_task("receiver", 0)
            .await
            .unwrap()
            .is_none(),
        "manual receive owns the exact canonical task"
    );
    let request = || Request::Reply {
        request_id: task.record.id.clone(),
        status: ReplyStatus::Done,
        message: "exact host result".into(),
    };
    assert_eq!(
        handle_in_state(None, &state, request(), origin("other"))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(
        handle_in_state(None, &state, request(), origin("host:automation:run-1288"))
            .await
            .is_err()
    );
    handle_in_state(None, &state, request(), origin("receiver"))
        .await
        .unwrap();
    handle_in_state(None, &state, request(), origin("receiver"))
        .await
        .unwrap();
    let reply = state
        .interactions
        .structured_reply(&task.record.id)
        .await
        .unwrap();
    assert_eq!(reply.source_session_id.as_deref(), Some("receiver"));
    assert_eq!(reply.body, "exact host result");
    assert_eq!(count("structured_replies"), 1);
    assert_eq!(
        count("agent_message_availability"),
        1,
        "host reply is durable without a fake recipient inbox"
    );
}

#[tokio::test]
async fn host_automation_native_claim_keeps_provenance_and_atomic_reply() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    let task = state
        .interactions
        .admit_host_automation_task("run-native", "build", "receiver", "work")
        .await
        .unwrap();
    let claim = state
        .interactions
        .claim_agent_task("receiver", 0)
        .await
        .unwrap()
        .unwrap();
    let frame: serde_json::Value =
        serde_json::from_str(&prepare_claim_context(&state, &claim).await.unwrap()).unwrap();
    assert_eq!(frame["host_automation"]["run_id"], "run-native");
    assert_eq!(frame["host_automation"]["node"], "build");
    assert!(state
        .interactions
        .receive_agent_messages("receiver", None, None, 100)
        .await
        .unwrap()
        .messages
        .is_empty());
    store::with_db(|conn| { conn.execute_batch("CREATE TRIGGER fail_host_reply BEFORE INSERT ON structured_replies BEGIN SELECT RAISE(ABORT,'injected'); END;")?; Ok(()) }).unwrap();
    assert!(state
        .interactions
        .reply_agent_message("receiver", &task.record.id, ReplyStatus::Done, "result")
        .await
        .is_err());
    assert_eq!(stored_status(&task.record.id), "awaiting_reply");
    assert!(state
        .interactions
        .structured_reply(&task.record.id)
        .await
        .is_none());
    store::with_db(|conn| {
        conn.execute_batch("DROP TRIGGER fail_host_reply")?;
        Ok(())
    })
    .unwrap();
    state
        .interactions
        .finish_agent_task(&claim, "provider_visible")
        .await
        .unwrap();
    state
        .interactions
        .reply_agent_message("receiver", &task.record.id, ReplyStatus::Done, "result")
        .await
        .unwrap();
    assert_eq!(
        state
            .interactions
            .structured_reply(&task.record.id)
            .await
            .unwrap()
            .body,
        "result"
    );
}
