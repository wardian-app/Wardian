use super::*;
use std::future::{poll_fn, Future};
use std::path::Path;
use std::task::Poll;
use std::time::{Duration, Instant};
use tauri::Manager;
use wardian_core::conversations::AgentConversationLoggingSetting;
use wardian_core::models::AgentConfig;

const HEADER: &str = "Current header before global work";

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
            text: HEADER.into(),
            provider: "mock".into(),
            turn_id: Some("current-turn".into()),
            source: Some("model".into()),
            provider_provenance: None,
        },
    );
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    app
}

fn snapshot_busy(error: &str) -> bool {
    matches!(
        error,
        "chat snapshot temporarily busy"
            | "chat config temporarily busy"
            | "chat status temporarily busy"
            | "chat source temporarily busy"
    )
}

async fn read_recent(
    state: &AppState,
    session: &str,
    revision: Option<String>,
) -> Result<wardian_core::models::chat::AgentChatPage, String> {
    load_agent_chat_page_with_recent_for_state(
        state,
        session.into(),
        None,
        revision,
        None,
        None,
        RecentChatRead {
            limit: Some(8),
            preview: true,
        },
    )
    .await
}

async fn ordinary_selection(
    state: &AppState,
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    session: &str,
) -> wardian_core::models::chat::AgentChatPage {
    loop {
        // Match normal command dispatch: request the coalesced producer before
        // consuming its slot. Only documented snapshot contention is retried.
        match queue_recent_capture_for_app(state, app, session) {
            Ok(()) => {}
            Err(error) if snapshot_busy(&error) => {
                tokio::task::yield_now().await;
                continue;
            }
            Err(error) => panic!("selection admission failed: {error}"),
        }
        match read_recent(state, session, None).await {
            Ok(page) if page.recent_admission.is_some() => return page,
            Ok(_) => {}
            Err(error) if snapshot_busy(&error) => {}
            Err(error) => panic!("selection read failed: {error}"),
        }
        tokio::task::yield_now().await;
    }
}

fn assert_preview(page: &wardian_core::models::chat::AgentChatPage) {
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].text.as_deref(), Some(HEADER));
    assert!(page.events[0].metadata["chat_source_ref"].is_null());
    assert!(page.recent_admission.is_some());
    assert!(page.conversation_id.is_none() && page.generation.is_none());
    assert!(page.source_epoch.is_none() && page.next_before.is_none());
    assert!(page.detail.is_none() && page.revalidated_ids.is_none());
    assert!(page.aliases.is_empty());
}

async fn producer_joined(state: &AppState) {
    while state
        .conversation_archive
        .chat_projection
        .recent_pending_for_test()
        != 0
    {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_selection_recovers_after_forced_snapshot_and_legacy_prelude_misses() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let session = "recent-selection-snapshot-miss";
    let app = fixture(home.path(), session).await;
    let state = app.state::<AppState>();
    let old_work = state.conversation_capture_policy_lock.lock().await;
    let roster = state.agents.lock().await;
    let mut capture = Box::pin(archive_agent_chat_events_for_state(&state, session));
    // The held roster deterministically rejects the one-shot legacy prelude.
    // Its real future then reaches the still-held old policy work.
    poll_fn(|cx| {
        assert!(capture.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        queue_recent_capture_for_app(&state, app.handle(), session).unwrap_err(),
        "chat snapshot temporarily busy"
    );
    assert_eq!(
        state
            .conversation_archive
            .chat_projection
            .recent_pending_for_test(),
        0
    );
    drop(roster);
    let started = Instant::now();
    let selected = tokio::time::timeout(Duration::from_secs(1), async {
        let page = ordinary_selection(&state, app.handle(), session).await;
        producer_joined(&state).await;
        (page, started.elapsed())
    })
    .await;
    drop(old_work);
    assert!(tokio::time::timeout(Duration::from_secs(10), capture)
        .await
        .unwrap()
        .is_ok());
    let (page, elapsed) =
        selected.expect("normal selection retries while old capture remains held");
    assert!(elapsed < Duration::from_secs(1));
    assert_preview(&page);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_selection_releases_failed_publication_and_coalesces_its_retry() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let session = "recent-selection-publication-miss";
    let app = fixture(home.path(), session).await;
    let state = app.state::<AppState>();
    let old_work = state.conversation_capture_policy_lock.lock().await;
    let config = state
        .agents
        .lock()
        .await
        .get(session)
        .unwrap()
        .config
        .clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    queue_recent_capture_for_app_using(&state, app.handle(), session, move |state, id, runtime| {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let result = publish_recent_for_state(state, id, runtime);
        result_tx
            .send(result.as_ref().map(|_| ()).map_err(ToString::to_string))
            .unwrap();
        result
    })
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    for _ in 0..8 {
        queue_recent_capture_for_app(&state, app.handle(), session).unwrap();
    }
    assert_eq!(
        state
            .conversation_archive
            .chat_projection
            .recent_pending_for_test(),
        1
    );
    let held = config.lock().unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap_err(),
        "chat config temporarily busy"
    );
    drop(held);
    let started = Instant::now();
    let selected = tokio::time::timeout(Duration::from_secs(1), async {
        producer_joined(&state).await;
        let page = ordinary_selection(&state, app.handle(), session).await;
        producer_joined(&state).await;
        (page, started.elapsed())
    })
    .await;
    let (page, elapsed) =
        selected.expect("a joined failed producer must leave selection retryable");
    assert!(elapsed < Duration::from_secs(1));
    assert_preview(&page);
    for _ in 0..8 {
        queue_recent_capture_for_app(&state, app.handle(), session).unwrap();
    }
    assert_eq!(
        state
            .conversation_archive
            .chat_projection
            .recent_pending_for_test(),
        0
    );
    let privacy = state
        .conversation_archive
        .chat_projection
        .recent_boundary(Some(session))
        .unwrap();
    queue_recent_capture_for_app(&state, app.handle(), session).unwrap();
    let retired = read_recent(&state, session, Some(page.revision))
        .await
        .unwrap();
    assert!(retired.reset && retired.events.is_empty() && retired.recent_admission.is_none());
    assert_eq!(
        state
            .conversation_archive
            .chat_projection
            .recent_pending_for_test(),
        0
    );
    drop(privacy);
    drop(old_work);
}
