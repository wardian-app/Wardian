// Receipt integration regression included in control::tests to share its fixture.
#[tokio::test]
async fn message_delivery_writes_terminal_bytes_after_opencode_is_ready() {
    let _home = TestWardianHome::new_async().await;
    let receipt_fixture = super::test_support::opencode_receipt_fixture(_home.path(), "ses_test");
    let opencode_db = receipt_fixture.db_path.clone();
    let state = AppState::new();
    insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
    {
        let agents = state.agents.lock().await;
        let agent = agents.get("agent-1").unwrap();
        agent.config.lock().unwrap().provider = "opencode".to_string();
        agent.config.lock().unwrap().resume_session = Some("ses_test".to_string());
        *agent.current_status.lock().unwrap() = "Idle".to_string();
        *agent.terminal_title.lock().unwrap() = "OpenCode".to_string();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    install_test_terminal_runtime(&state, "agent-1", tx).await;
    set_test_opencode_screen(&state, "OpenCode", OPENCODE_READY_SCREEN).await;

    let receipt_db = opencode_db.clone();
    let (submitted_tx, submitted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let receiver = tokio::spawn(async move {
        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\x1b[13u".to_vec());
        submitted_tx.send(()).expect("signal submitted payload");
        release_rx.await.expect("release receipt insert");
        tokio::task::spawn_blocking(move || {
            super::test_support::insert_opencode_user_receipt(
                &receipt_db,
                "ses_test",
                "message-1",
                "part-1",
                "hello",
            );
        })
        .await
        .expect("insert OpenCode receipt");
    });

    let delivery = deliver_message_to_target(
        None,
        &state,
        "OpenCodeOne",
        "hello",
        None,
        MessageInputMode::Message,
        QueuePolicy::QueueIfBusy,
        None,
        None,
        false,
    );
    tokio::pin!(delivery);
    tokio::select! {
        result = &mut delivery => panic!("delivery completed before the OpenCode receipt: {result:?}"),
        _ = submitted_rx => {
            tokio::select! {
                result = &mut delivery => panic!("delivery completed before the OpenCode receipt: {result:?}"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
        }
    }
    release_tx.send(()).expect("release receipt insert");
    delivery.await.unwrap();

    receiver.await.expect("receive OpenCode input");
}
