use super::*;
use std::future::{poll_fn, Future};
use std::path::Path;
use std::task::Poll;
use std::time::Duration;
use tauri::Manager;
use wardian_core::conversations::AgentConversationLoggingSetting;
use wardian_core::models::AgentConfig;

const HEADER: &str = "Owned archive answer";

async fn fixture(home: &Path, session: &str) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let mut config = AgentConfig {
        session_id: session.into(),
        session_name: "Recent selection contention".into(),
        agent_class: "Coder".into(),
        provider: "mock".into(),
        folder: home.to_string_lossy().into_owned(),
        is_off: true,
        conversation_logging: AgentConversationLoggingSetting::Enabled,
        ..Default::default()
    };
    config.reset_provider_config_for_provider();
    let agent =
        crate::restored_agent_without_process(config.clone(), "Off", String::new(), None, None);
    agent.watch_state.lock().unwrap().push_transcript(
        wardian_core::control::WatchTranscriptMessage {
            role: "assistant".into(),
            text: "Unarchived Watch answer".into(),
            provider: "mock".into(),
            turn_id: Some("current-turn".into()),
            source: Some("model".into()),
            provider_provenance: None,
        },
    );
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    let (snapshot, _) = chat_read_snapshot_for_state(&state, session).unwrap();
    state
        .conversation_archive
        .append_delivered_input_with_context(
            conversation_archive_context_from_snapshot(&snapshot),
            HEADER,
            None,
        )
        .unwrap();
    app
}

async fn read_recent(
    state: &AppState,
    session: &str,
) -> Result<wardian_core::models::chat::AgentChatPage, String> {
    load_agent_chat_page_with_preview_for_state(
        state,
        session.into(),
        None,
        None,
        None,
        None,
        RecentChatRead {
            limit: Some(8),
            preview: true,
        },
    )
    .await
}

fn assert_preview(page: &wardian_core::models::chat::AgentChatPage) {
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].text.as_deref(), Some(HEADER));
    assert!(page.events[0].metadata["chat_source_ref"].is_null());
    assert!(page.recent_admission.is_none());
    assert!(page.conversation_id.is_some() && page.generation.is_some());
    assert!(page.revision.starts_with("archive1:"));
    assert!(page.source_epoch.is_none() && page.next_before.is_none());
    assert!(page.detail.is_none() && page.revalidated_ids.is_none());
    assert!(page.aliases.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_selection_recovers_after_snapshot_contention_without_capture() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let session = "recent-selection-snapshot-miss";
    let app = fixture(home.path(), session).await;
    let state = app.state::<AppState>();
    let old_work = state.conversation_capture_policy_lock.lock().await;
    let roster = state.agents.lock().await;
    assert_eq!(
        read_recent(&state, session).await.unwrap_err(),
        "chat snapshot temporarily busy"
    );
    drop(roster);
    let page = tokio::time::timeout(Duration::from_secs(10), read_recent(&state, session))
        .await
        .expect("owned reads proceed while the capture policy lock is held")
        .unwrap();
    drop(old_work);
    assert_preview(&page);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_selection_withholds_a_read_after_runtime_replacement() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let session = "recent-selection-publication-miss";
    let app = fixture(home.path(), session).await;
    let state = app.state::<AppState>();
    let gate = state
        .conversation_archive
        .test_agent_archive_gate(session)
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let holder = std::thread::spawn(move || {
        let _guard = gate.lock().unwrap();
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut read = Box::pin(read_recent(&state, session));
    poll_fn(|cx| {
        // Admission is synchronous up to the blocked archive read.
        assert!(read.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    state
        .agents
        .lock()
        .await
        .get_mut(session)
        .unwrap()
        .current_status = Arc::new(Mutex::new("Off".into()));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), read)
            .await
            .unwrap()
            .unwrap_err(),
        "chat runtime changed during read"
    );
    assert_preview(&read_recent(&state, session).await.unwrap());
}
