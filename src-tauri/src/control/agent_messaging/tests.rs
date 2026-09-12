//! Messaging authorization, ownership, correlation, and queue progression tests.
use super::super::test_support::TestWardianHome;
use super::*;
use wardian_core::agent_messaging::{AgentMessagePage, TaskDeliveryOwner};

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
async fn legacy_reply_to_v2_task_uses_atomic_available_reply() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender", "Sender").await;
    agent(&state, "receiver", "Receiver").await;
    let task = task(&state).await;
    let request_id = &task.record.id;
    for caller in [None, Some(origin("sender"))] {
        assert_eq!(
            submit_structured_reply(
                &state,
                request_id,
                ReplyStatus::Done,
                "legacy\nλ",
                caller.as_ref(),
                None
            )
            .await
            .unwrap_err()
            .code,
            "unauthorized"
        );
    }
    store::with_db(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_reply_available BEFORE INSERT ON agent_message_availability BEGIN SELECT RAISE(ABORT,'injected'); END;")?; Ok(())
    }).unwrap();
    assert!(submit_structured_reply(
        &state,
        request_id,
        ReplyStatus::Done,
        "legacy\nλ",
        Some(&origin("receiver")),
        None
    )
    .await
    .is_err());
    assert_eq!(stored_status(request_id), "awaiting_reply");
    assert_eq!(
        state.interactions.interaction(request_id).await.unwrap(),
        task.record
    );
    assert!(state
        .interactions
        .structured_reply(request_id)
        .await
        .is_none());
    assert_eq!(count("interactions"), 1);
    assert_eq!(count("structured_replies"), 0);
    assert_eq!(count("agent_message_availability"), 1);
    store::with_db(|conn| {
        conn.execute_batch("DROP TRIGGER fail_reply_available")?;
        Ok(())
    })
    .unwrap();
    let reply = submit_structured_reply(
        &state,
        request_id,
        ReplyStatus::Done,
        "legacy\nλ",
        Some(&origin("receiver")),
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.request_id, *request_id);
    assert_eq!(reply.source_session_id.as_deref(), Some("receiver"));
    assert_eq!(reply.body, "legacy\nλ");
    assert_eq!(stored_status(request_id), "completed");
    assert_eq!(
        state
            .interactions
            .structured_reply(request_id)
            .await
            .unwrap(),
        reply
    );
    let page = receive(&state, "sender").await;
    assert_eq!(page.messages.len(), 1);
    assert_eq!(
        page.messages[0].parent_interaction_id.as_deref(),
        Some(request_id.as_str())
    );
    assert_eq!(page.messages[0].message, reply.body);
    assert_eq!(
        submit_structured_reply(
            &state,
            request_id,
            ReplyStatus::Done,
            "legacy\nλ",
            Some(&origin("receiver")),
            None
        )
        .await
        .unwrap(),
        reply
    );
    assert_eq!(
        submit_structured_reply(
            &state,
            request_id,
            ReplyStatus::Done,
            "different",
            Some(&origin("receiver")),
            None
        )
        .await
        .unwrap_err()
        .code,
        "conflicting_reply"
    );
    assert_eq!(count("structured_replies"), 1);
    assert_eq!(count("interactions"), 2);
    assert_eq!(receive(&state, "sender").await, page);
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
