// Receipt integration regression included in control::tests to share its fixture.
#[tokio::test]
async fn message_delivery_writes_terminal_bytes_after_opencode_is_ready() {
    let _home = TestWardianHome::new_async().await;
    let xdg_data_home = _home.path().join("xdg-data");
    let opencode_dir = xdg_data_home.join("opencode");
    std::fs::create_dir_all(&opencode_dir).expect("create OpenCode fixture directory");
    let opencode_db = opencode_dir.join("opencode.db");
    {
        let connection =
            rusqlite::Connection::open(&opencode_db).expect("create OpenCode fixture database");
        connection
            .execute_batch(
                r#"
                CREATE TABLE message (
                    id text PRIMARY KEY,
                    session_id text NOT NULL,
                    time_created integer,
                    time_updated integer,
                    data text NOT NULL
                );
                CREATE TABLE part (
                    id text PRIMARY KEY,
                    message_id text NOT NULL,
                    session_id text NOT NULL,
                    time_created integer,
                    time_updated integer,
                    data text NOT NULL
                );
                "#,
            )
            .expect("create OpenCode fixture schema");
    }
    let _xdg_data_home =
        ScopedEnvVar::set("XDG_DATA_HOME", xdg_data_home.to_string_lossy().as_ref());
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

    let receipt_db = opencode_db.clone();
    let (submitted_tx, submitted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let receiver = tokio::spawn(async move {
        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\x1b[13u".to_vec());
        submitted_tx.send(()).expect("signal submitted payload");
        release_rx.await.expect("release receipt insert");
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(receipt_db)
                .expect("open OpenCode receipt database");
            connection
                .execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, 1, 1, ?3)",
                    rusqlite::params!["message-1", "ses_test", r#"{"role":"user"}"#],
                )
                .expect("insert OpenCode user message");
            connection
                .execute(
                    "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, 2, 2, ?4)",
                    rusqlite::params![
                        "part-1",
                        "message-1",
                        "ses_test",
                        r#"{"type":"text","text":"hello"}"#,
                    ],
                )
                .expect("insert OpenCode user part");
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
