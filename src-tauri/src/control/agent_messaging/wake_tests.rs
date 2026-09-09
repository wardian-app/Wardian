use super::super::test_support::TestWardianHome;
use super::*;

async fn agent(state: &AppState, id: &str) {
    let agent = super::super::tests::test_agent(id, id, "Test");
    *agent.current_status.lock().unwrap() = "Off".into();
    state.agents.lock().await.insert(id.into(), agent);
}

fn receive_request(timeout_ms: u64) -> Request {
    Request::ReceiveMessages {
        cursor: None,
        ack_cursor: None,
        limit: None,
        timeout_ms: Some(timeout_ms),
    }
}

#[tokio::test]
async fn pending_receive_wakes_on_committed_native_reply_without_duplicate_body_or_ack() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender").await;
    agent(&state, "receiver").await;
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
    let reply = state
        .interactions
        .reply_agent_message(
            "receiver",
            &task.record.id,
            ReplyStatus::Done,
            "exact reply\nλ😀",
        )
        .await
        .unwrap();
    // Native owns the body before receive starts, but has no receipt yet.
    let claim = state
        .interactions
        .claim_agent_information("sender", &reply.record.id, 0)
        .await
        .unwrap()
        .unwrap();
    let empty = state
        .interactions
        .receive_agent_messages("sender", None, None, 100)
        .await
        .unwrap();
    assert!(empty.messages.is_empty());
    let origin = || MessageOrigin::WardianAgent {
        session_id: "sender".into(),
    };
    let receive = handle_in_state(None, &state, receive_request(60_000), origin());
    tokio::pin!(receive);
    // Poll the actual handler through its empty-page read to its first wait.
    // This is a deterministic boundary, without a sleep or provider substitute.
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(receive.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    store::with_db(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_wake_receipt BEFORE UPDATE ON agent_message_delivery BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        Ok(())
    }).unwrap();
    assert!(state
        .interactions
        .finish_agent_task(&claim, "provider_accepted")
        .await
        .is_err());
    assert_eq!(
        state
            .interactions
            .agent_message_provider_revision("sender")
            .await,
        0
    );
    store::with_db(|conn| {
        conn.execute_batch("DROP TRIGGER fail_wake_receipt")?;
        Ok(())
    })
    .unwrap();
    state
        .interactions
        .finish_agent_task(&claim, "provider_accepted")
        .await
        .unwrap();
    assert_eq!(
        state
            .interactions
            .agent_message_provider_revision("receiver")
            .await,
        0
    );
    let response = tokio::time::timeout(Duration::from_secs(1), receive)
        .await
        .expect("native receipt must wake the 60-second receive")
        .unwrap();
    let Response::ReceiveMessages { page } = response else {
        panic!("receive page")
    };
    assert!(page.messages.is_empty());
    assert!(!page.timed_out);
    assert_eq!(
        page.wake_reason.as_deref(),
        Some("provider_context_available")
    );
    assert_eq!(page.next_cursor, empty.next_cursor);
    assert_eq!(page.ack_cursor, empty.ack_cursor);
    assert!(state
        .interactions
        .claim_agent_information("sender", &reply.record.id, 0)
        .await
        .unwrap()
        .is_none());
    let acknowledgements = store::with_db(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM agent_message_ack WHERE recipient='sender'",
            [],
            |row| row.get::<_, i64>(0),
        )?)
    })
    .unwrap();
    assert_eq!(acknowledgements, 0);
    // Old receipts do not spuriously wake a subsequent call.
    let response = handle_in_state(None, &state, receive_request(5), origin())
        .await
        .unwrap();
    assert!(
        matches!(response, Response::ReceiveMessages { page } if page.timed_out && page.wake_reason.is_none() && page.messages.is_empty())
    );
}

#[tokio::test]
async fn uncertain_native_delivery_does_not_publish_provider_context_wake() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    agent(&state, "sender").await;
    agent(&state, "receiver").await;
    let info = state
        .interactions
        .admit_agent_message(store::Admission {
            sender: "sender",
            recipient: "receiver",
            message: "information",
            idempotency_key: None,
            task: false,
            generation: 0,
        })
        .await
        .unwrap();
    let claim = state
        .interactions
        .claim_agent_information("receiver", &info.record.id, 0)
        .await
        .unwrap()
        .unwrap();
    state
        .interactions
        .finish_agent_task(&claim, "uncertain")
        .await
        .unwrap();
    assert_eq!(
        state
            .interactions
            .agent_message_provider_revision("receiver")
            .await,
        0
    );
    assert!(state
        .interactions
        .receive_agent_messages("receiver", None, None, 100)
        .await
        .unwrap()
        .messages
        .is_empty());
}
