//! Exercise the real control/interaction boundary with an isolated canonical DB.
use super::*;
use crate::control::test_support::TestWardianHome;
use wardian_core::control::{InteractionKind, InteractionStatus, ReplyStatus};

const OWNER: &str = "root-agent";
const REQUESTER: &str = "requester";
const ROOT: &str = "root-thread";

async fn fixture() -> (AppState, String, u64) {
    let state = AppState::new();
    for id in [OWNER, REQUESTER] {
        let agent = crate::control::tests::test_agent(id, id, "Test");
        *agent.current_status.lock().unwrap() = "Off".into();
        agent.config.lock().unwrap().provider = "codex".into();
        if id == OWNER {
            agent.config.lock().unwrap().resume_session = Some(ROOT.into());
        }
        state.agents.lock().await.insert(id.into(), agent);
    }
    let generation = state
        .interactions
        .start_provider_input_generation(OWNER, ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let admitted = handle_in_state(
        None,
        &state,
        Request::FollowupTask {
            target: OWNER.into(),
            message: "root owns the whole task".into(),
            idempotency_key: None,
        },
        origin(REQUESTER, None),
    )
    .await
    .unwrap();
    let Response::FollowupTask { request_id, .. } = admitted else {
        panic!("task response")
    };
    store::with_db(|conn| {
        let claim = store::claim_next_task(conn, OWNER, generation)?.unwrap();
        store::bind_task_turn(conn, &claim, "codex", ROOT, "root-turn", "start")
    })
    .unwrap();
    (state, request_id, generation)
}

// JSON keeps this reproducer executable against the prior wire model too:
// older MessageOrigin silently drops the provider_thread_id member.
fn origin(agent: &str, thread: Option<&str>) -> MessageOrigin {
    serde_json::from_value(serde_json::json!({
        "kind":"wardian_agent", "session_id":agent, "provider_thread_id":thread,
    }))
    .unwrap()
}

#[tokio::test]
async fn fork_explicit_reply_is_rejected_and_retained_only_as_parent_progress() {
    let _home = TestWardianHome::new_async().await;
    let (state, request_id, _) = fixture().await;
    let result = handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "helper slice complete".into(),
        },
        origin(OWNER, Some("fork-thread")),
    )
    .await;
    assert!(
        result.is_err(),
        "a fork's inherited Wardian identity must not settle its parent's task"
    );
    assert_eq!(
        store::with_db(|conn| store::load(conn, &request_id))
            .unwrap()
            .status,
        InteractionStatus::AwaitingReply
    );
    assert!(state
        .interactions
        .structured_reply(&request_id)
        .await
        .is_none());
    let requester =
        store::with_db(|conn| store::receive(conn, REQUESTER, None, None, 100)).unwrap();
    assert!(
        requester.messages.is_empty(),
        "child progress must not impersonate the parent to its requester"
    );
    let parent = store::with_db(|conn| store::receive(conn, OWNER, None, None, 100)).unwrap();
    assert!(parent
        .messages
        .iter()
        .all(|message| message.kind == InteractionKind::Message && message.reply_status.is_none()));
    assert!(parent
        .messages
        .iter()
        .any(|message| message.message.contains("fork-thread")));
    assert!(parent
        .messages
        .iter()
        .any(|message| message.message == "helper slice complete"));
    let error = result.err().unwrap();
    assert_eq!(error.code, "foreign_provider_thread");
    assert!(error.message.contains("owning parent"));
    assert!(error.details.unwrap()["progress_interaction_id"].is_string());
    assert!(handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "helper slice complete".into(),
        },
        origin(OWNER, Some("fork-thread"))
    )
    .await
    .is_err());
    let replay = store::with_db(|conn| store::receive(conn, OWNER, None, None, 100)).unwrap();
    assert_eq!(
        replay.messages, parent.messages,
        "rejected settlement retries cannot duplicate progress"
    );
    handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "root finished the whole task".into(),
        },
        origin(OWNER, Some(ROOT)),
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .interactions
            .structured_reply(&request_id)
            .await
            .unwrap()
            .body,
        "root finished the whole task"
    );
}

#[tokio::test]
async fn root_explicit_reply_still_settles_its_assigned_task() {
    let _home = TestWardianHome::new_async().await;
    let (state, request_id, _) = fixture().await;
    let result = handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "root complete".into(),
        },
        origin(OWNER, Some(ROOT)),
    )
    .await
    .unwrap();
    assert!(matches!(result, Response::Reply { .. }));
    assert_eq!(
        state
            .interactions
            .structured_reply(&request_id)
            .await
            .unwrap()
            .body,
        "root complete"
    );
}

#[tokio::test]
async fn missing_thread_provenance_settles_with_a_visible_diagnostic() {
    let _home = TestWardianHome::new_async().await;
    let (state, request_id, _) = fixture().await;
    handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "legacy complete".into(),
        },
        origin(OWNER, None),
    )
    .await
    .unwrap();
    assert!(state
        .interactions
        .structured_reply(&request_id)
        .await
        .is_some());
    let attempts = store::with_db(|conn| {
        Ok(wardian_core::db::list_interaction_delivery_attempts_with_conn(conn, &request_id)?)
    })
    .unwrap();
    assert!(attempts
        .iter()
        .any(|attempt| attempt.delivery_state == "thread_provenance_unavailable"));
}

#[tokio::test]
async fn same_thread_from_a_retired_generation_cannot_settle() {
    let _home = TestWardianHome::new_async().await;
    let (state, request_id, old_generation) = fixture().await;
    let current = state
        .interactions
        .start_provider_input_generation(OWNER, ProviderInputReadiness::Ready, None)
        .await
        .generation;
    assert_ne!(current, old_generation);
    let result = handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "late output".into(),
        },
        origin(OWNER, Some(ROOT)),
    )
    .await;
    assert!(
        result.is_err(),
        "thread equality cannot bless a retired runtime generation"
    );
    assert!(state
        .interactions
        .structured_reply(&request_id)
        .await
        .is_none());
}

fn seed_child(
    home: &TestWardianHome,
    child: &str,
    parent: &str,
    parent_worker: Option<&str>,
) -> wardian_core::temporary_workers::TemporaryWorkerRecord {
    use wardian_core::temporary_workers as workers;
    let source = home.path().join(format!("rollout-{child}.jsonl"));
    std::fs::write(&source, format!("{}\n", serde_json::json!({
        "type":"session_meta","payload":{"id":child,"source":{"subagent":{"thread_spawn":{"parent_thread_id":parent}}}}
    }))).unwrap();
    workers::observe_codex_provider_child(workers::ObserveCodexProviderChild {
        registration: workers::RegisterProviderChild {
            provider: "codex",
            workspace: home.path().to_str().unwrap(),
            root_agent_id: Some(OWNER),
            parent_worker_id: parent_worker,
            parent_provider_session_id: parent,
            runtime_session_id: OWNER,
            provider_session_id: child,
            automation_origin: None,
            state: workers::TemporaryWorkerState::Running,
            outcome: None,
            source_path: source.to_str().unwrap(),
            coverage: "codex_rollout_verified",
            requested_at: None,
            terminal_at: None,
        },
        qualification: workers::CodexChildObservationQualification::Qualified,
        turn_started_at: None,
    })
    .unwrap()
}

#[tokio::test]
async fn verified_nested_child_progress_reaches_parent_with_exact_body_and_lineage() {
    let home = TestWardianHome::new_async().await;
    let (state, _, generation) = fixture().await;
    let first = seed_child(&home, "first-child", ROOT, None);
    let nested = seed_child(&home, "nested-child", "first-child", Some(&first.worker_id));
    assert_eq!(
        nested.runtime_generation, None,
        "provider children have parentage, not an invented Wardian runtime epoch"
    );
    let body = "λ".repeat(32_768);
    for duplicate in [false, true] {
        let result = handle_in_state(
            None,
            &state,
            Request::SendMessage {
                target: REQUESTER.into(),
                message: body.clone(),
                idempotency_key: Some("child-progress-call".into()),
            },
            origin(OWNER, Some("nested-child")),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Response::SendMessage { duplicate: actual, .. } if actual == duplicate)
        );
    }
    let requester =
        store::with_db(|conn| store::receive(conn, REQUESTER, None, None, 100)).unwrap();
    assert!(requester.messages.is_empty());
    let parent = store::with_db(|conn| store::receive(conn, OWNER, None, None, 100)).unwrap();
    assert_eq!(parent.messages.len(), 2);
    assert!(parent.messages[0].message.contains("nested-child"));
    assert!(parent.messages[0]
        .message
        .contains("parent thread first-child"));
    assert_eq!(parent.messages[1].message.as_bytes(), body.as_bytes());
    assert!(parent
        .messages
        .iter()
        .all(|message| message.reply_status.is_none()));
    let origin =
        store::with_db(|conn| store::recipient_origin(conn, &parent.messages[1].interaction_id))
            .unwrap()
            .unwrap();
    assert_eq!(origin.conversation_id, ROOT);
    assert_eq!(origin.generation, generation);
}

#[tokio::test]
async fn a_root_information_send_keeps_its_original_destination() {
    let _home = TestWardianHome::new_async().await;
    let (state, _, _) = fixture().await;
    handle_in_state(
        None,
        &state,
        Request::SendMessage {
            target: REQUESTER.into(),
            message: "root progress".into(),
            idempotency_key: None,
        },
        origin(OWNER, Some(ROOT)),
    )
    .await
    .unwrap();
    let requester =
        store::with_db(|conn| store::receive(conn, REQUESTER, None, None, 100)).unwrap();
    assert_eq!(requester.messages.len(), 1);
    assert_eq!(requester.messages[0].message, "root progress");
}

#[tokio::test]
async fn child_progress_does_not_bypass_exact_target_validation() {
    let _home = TestWardianHome::new_async().await;
    let (state, _, _) = fixture().await;
    let error = handle_in_state(
        None,
        &state,
        Request::SendMessage {
            target: "all".into(),
            message: "invalid target".into(),
            idempotency_key: None,
        },
        origin(OWNER, Some("fork-thread")),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "invalid_target");
    let parent = store::with_db(|conn| store::receive(conn, OWNER, None, None, 100)).unwrap();
    assert!(parent.messages.is_empty());
}

#[tokio::test]
async fn another_provider_retains_its_existing_reply_contract() {
    let _home = TestWardianHome::new_async().await;
    let (state, request_id, _) = fixture().await;
    state
        .agents
        .lock()
        .await
        .get(OWNER)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .provider = "claude".into();
    handle_in_state(
        None,
        &state,
        Request::Reply {
            request_id: request_id.clone(),
            status: ReplyStatus::Done,
            message: "other provider".into(),
        },
        origin(OWNER, Some("unrelated-codex-environment")),
    )
    .await
    .unwrap();
    assert!(state
        .interactions
        .structured_reply(&request_id)
        .await
        .is_some());
    let attempts = store::with_db(|conn| {
        Ok(wardian_core::db::list_interaction_delivery_attempts_with_conn(conn, &request_id)?)
    })
    .unwrap();
    assert!(!attempts
        .iter()
        .any(|attempt| attempt.delivery_phase.as_deref() == Some("caller_thread_validation")));
}

#[tokio::test]
async fn send_before_root_publication_is_retriable_and_creates_no_progress() {
    let _home = TestWardianHome::new_async().await;
    let (state, _, _) = fixture().await;
    let config = state.agents.lock().await.get(OWNER).unwrap().config.clone();
    config.lock().unwrap().resume_session = None;
    let request = Request::SendMessage {
        target: REQUESTER.into(),
        message: "root starting up".into(),
        idempotency_key: Some("root-send".into()),
    };
    let error = handle_in_state(None, &state, request.clone(), origin(OWNER, Some(ROOT)))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "provider_thread_unavailable");
    for recipient in [OWNER, REQUESTER] {
        let page = store::with_db(|conn| store::receive(conn, recipient, None, None, 100)).unwrap();
        assert!(page.messages.is_empty());
    }
    config.lock().unwrap().resume_session = Some(ROOT.into());
    let reply = handle_in_state(None, &state, request, origin(OWNER, Some(ROOT)))
        .await
        .unwrap();
    assert!(matches!(
        reply,
        Response::SendMessage {
            duplicate: false,
            ..
        }
    ));
    let page = store::with_db(|conn| store::receive(conn, REQUESTER, None, None, 100)).unwrap();
    assert_eq!(page.messages[0].message, "root starting up");
}

#[tokio::test]
async fn reset_keeps_progress_in_history_without_delivering_into_the_replacement() {
    let home = TestWardianHome::new_async().await;
    let (state, request_id, _) = fixture().await;
    seed_child(&home, "fork-thread", ROOT, None);
    let foreign_reply = |body: &str| Request::Reply {
        request_id: request_id.clone(),
        status: ReplyStatus::Done,
        message: body.into(),
    };
    let before = handle_in_state(
        None,
        &state,
        foreign_reply("before reset"),
        origin(OWNER, Some("fork-thread")),
    )
    .await
    .err()
    .unwrap();
    let id_before = before.details.as_ref().unwrap()["progress_interaction_id"]
        .as_str()
        .unwrap();
    assert!(
        store::with_db(|conn| store::pending_wakeable_information(conn, OWNER))
            .unwrap()
            .contains(&id_before.to_owned())
    );
    state
        .interactions
        .fence_agent_conversation_deliveries(OWNER)
        .await
        .unwrap();
    state
        .interactions
        .start_provider_input_generation(OWNER, ProviderInputReadiness::Ready, None)
        .await;
    state
        .agents
        .lock()
        .await
        .get(OWNER)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .resume_session = Some("new-root".into());
    let after = handle_in_state(
        None,
        &state,
        foreign_reply("after reset"),
        origin(OWNER, Some("fork-thread")),
    )
    .await
    .err()
    .unwrap();
    let id_after = after.details.as_ref().unwrap()["progress_interaction_id"]
        .as_str()
        .unwrap();
    let send = handle_in_state(
        None,
        &state,
        Request::SendMessage {
            target: REQUESTER.into(),
            message: "late child message".into(),
            idempotency_key: None,
        },
        origin(OWNER, Some("fork-thread")),
    )
    .await
    .unwrap();
    let Response::SendMessage {
        interaction_id: late,
        delivery_state,
        ..
    } = send
    else {
        panic!("expected a progress receipt");
    };
    assert_eq!(delivery_state, "blocked_stale_origin");
    store::with_db(|conn| {
        assert!(store::pending_wakeable_information(conn, OWNER)?.is_empty());
        assert!(store::pending_information(
            conn,
            OWNER,
            store::information_highwater(conn, OWNER)?
        )?
        .is_empty());
        for (id, body) in [
            (id_before, "before reset"),
            (id_after, "after reset"),
            (&late, "late child message"),
        ] {
            assert_eq!(
                store::load(conn, id)?.body_ref,
                wardian_core::control::InteractionBodyRef::Inline { body: body.into() }
            );
        }
        Ok(())
    })
    .unwrap();
    assert!(state
        .interactions
        .structured_reply(&request_id)
        .await
        .is_none());
}
