// Included in control::tests to share the isolated terminal fixtures.

#[tokio::test]
async fn opencode_control_send_waits_for_current_composer_despite_cached_ready() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    install_test_terminal_runtime(&state, "agent-1", tx).await;
    set_test_opencode_screen(&state, "OpenCode", "Loading session...").await;
    record_provider_ready_evidence(&state, "agent-1", 0, ProviderReadyEvidence::ProviderEvent)
        .await;
    let info = delivery_target_infos(&state, &["agent-1".to_string()])
        .await
        .unwrap()
        .remove(0);

    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(50),
        wait_for_terminal_ready_for_control_send(&state, &info),
    )
    .await
    .is_err());
    set_test_opencode_screen(&state, "OpenCode", OPENCODE_READY_SCREEN).await;
    wait_for_terminal_ready_for_control_send(&state, &info)
        .await
        .unwrap();
}

#[tokio::test]
async fn opencode_control_send_accepts_current_composer_with_idle_oc_title() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    install_test_terminal_runtime(&state, "agent-1", tx).await;
    set_test_opencode_screen(&state, "OC | Self-introduction", OPENCODE_READY_SCREEN).await;
    let info = delivery_target_infos(&state, &["agent-1".to_string()])
        .await
        .unwrap()
        .remove(0);

    wait_for_terminal_ready_for_control_send(&state, &info)
        .await
        .unwrap();
}

#[tokio::test]
async fn message_delivery_writes_terminal_bytes_after_opencode_startup_is_ready() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    install_test_terminal_runtime(&state, "agent-1", tx).await;
    set_test_opencode_screen(&state, "OpenCode", OPENCODE_READY_SCREEN).await;

    deliver_message_to_target(
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
    )
    .await
    .unwrap();

    assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
    assert_eq!(rx.recv().await.unwrap(), b"\x1b[13u".to_vec());
}

const OPENCODE_READY_SCREEN: &str = "Ask anything...\nBuild  mimo-v2.5-free\nctrl+p commands";

async fn set_test_opencode_screen(state: &AppState, title: &str, screen: &str) {
    let (config, status, terminal_title, generation) = {
        let mut agents = state.agents.lock().await;
        let agent = agents.get_mut("agent-1").unwrap();
        agent.process_id = None;
        (
            agent.config.clone(),
            agent.current_status.clone(),
            agent.terminal_title.clone(),
            agent.runtime_generation.unwrap(),
        )
    };
    config.lock().unwrap().provider = "opencode".to_string();
    *status.lock().unwrap() = "Idle".to_string();
    *terminal_title.lock().unwrap() = title.to_string();
    let terminal = state.terminal_sessions.clone();
    let bytes = format!("\x1b[2J\x1b[H{}", screen.replace('\n', "\r\n")).into_bytes();
    tokio::task::spawn_blocking(move || {
        terminal.process_output_blocking("agent-1", generation, bytes)
    })
    .await
    .unwrap()
    .unwrap();
}
