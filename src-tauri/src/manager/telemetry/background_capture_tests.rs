use super::*;
use std::io::Write;

fn stopped_sample(
    _: Arc<tokio::sync::Mutex<sysinfo::System>>,
    _: Vec<String>,
    _: Vec<(String, Option<u32>)>,
) -> Option<SystemProcessSnapshot> {
    Some(SystemProcessSnapshot {
        logical_cpu_count: 1,
        children_map: Arc::new(HashMap::new()),
        processes: Arc::new(HashMap::new()),
        sys_refresh: std::time::Duration::ZERO,
        #[cfg(windows)]
        session_roots: HashMap::new(),
    })
}

async fn dispatch_ordinary_capture(state: &Arc<AppState>) -> usize {
    let (_, follow_up) = collect_agent_metrics_with_sampler(state, stopped_sample).await;
    assert!(
        follow_up.wake_sessions.is_empty(),
        "capture must not wake a provider"
    );
    let count = follow_up.background_captures.len();
    let mut workers = Vec::new();
    status::dispatch_capture_follow_up(state, follow_up.background_captures, |claim| {
        let state = state.clone();
        workers.push(tokio::spawn(async move {
            crate::commands::background_capture::run_background_capture(&state, claim).await;
        }));
    })
    .await;
    for worker in workers {
        worker.await.expect("ordinary capture worker");
    }
    count
}

async fn registered_off_source(
    home: &crate::control::test_support::TestWardianHome,
) -> (Arc<AppState>, String, std::path::PathBuf) {
    let session_id = "ordinary-off-capture".to_string();
    let conversation = "off-provider-conversation";
    let source_dir = home
        .path()
        .join("agents")
        .join(&session_id)
        .join("habitat/.codex/sessions/2026/10/04");
    std::fs::create_dir_all(&source_dir).unwrap();
    let path = source_dir.join(format!("rollout-2026-10-04T00-00-00-{conversation}.jsonl"));
    let header = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": conversation, "cwd": home.path() }
    });
    std::fs::write(&path, format!("{header}\n")).unwrap();
    let state = Arc::new(AppState::new());
    let agent = super::tests::test_active_agent(&session_id, "codex", "Off", None);
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.session_name = session_id.clone();
        config.is_off = true;
        config.resume_session = Some(conversation.into());
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    assert!(agent.log_path.lock().unwrap().is_none());
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .expect("ordinary production registration/persistence");
    (state, session_id, path)
}

fn append_records(path: &std::path::Path, count: usize) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for index in 0..count {
        let record = serde_json::json!({
            "type": "event_msg",
            "timestamp": "2026-10-04T12:00:00Z",
            "payload": {
                "type": "user_message",
                "message": format!("ordinary Off append {index}: {}", "x".repeat(512)),
            }
        });
        writeln!(file, "{record}").unwrap();
    }
}

fn capture_state(
    state: &AppState,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
) -> crate::commands::provider_log_acquisition::ProviderLogCaptureState {
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    state
        .conversation_archive
        .provider_log_capture_state(
            &snapshot.session_id,
            context.provider_source_key.as_deref().unwrap(),
        )
        .unwrap()
        .expect("ordinary capture published cursor")
}

#[tokio::test]
async fn ordinary_off_registration_lookup_and_follow_up_capture_without_chat_read() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    assert_eq!(dispatch_ordinary_capture(&state).await, 1);
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    assert_eq!(
        snapshot.log_path.as_deref(),
        Some(path.as_path()),
        "production private lookup"
    );
    let header_cursor = capture_state(&state, &snapshot);
    assert_eq!(
        header_cursor.committed_offset,
        std::fs::metadata(&path).unwrap().len()
    );
    assert!(
        header_cursor.unknown_before_offset.is_some(),
        "prime excludes unknown history"
    );
    append_records(&path, 500);
    let expected_end = std::fs::metadata(&path).unwrap().len();
    assert!(
        expected_end - header_cursor.committed_offset
            > crate::commands::provider_log_acquisition::PROVIDER_LOG_BATCH_BYTES
    );
    assert_eq!(dispatch_ordinary_capture(&state).await, 1);
    let captured = capture_state(&state, &snapshot);
    assert_eq!(
        captured.committed_offset, expected_end,
        "background drains all bounded batches"
    );
    assert_eq!(captured.status, "complete");
    let events = state
        .conversation_archive
        .chat_events_for_agent(&session_id)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event
                .text
                .as_deref()
                .is_some_and(|text| text.starts_with("ordinary Off append ")))
            .count(),
        500
    );
    let roster = state.agents.lock().await;
    let agent = roster.get(&session_id).unwrap();
    assert_eq!(*agent.current_status.lock().unwrap(), "Off");
    assert!(
        agent.runtime_generation.is_none(),
        "no provider was submitted"
    );
    drop(roster);
    let (_, follow_up) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    let mut launches = 0;
    status::dispatch_capture_follow_up(&state, follow_up.background_captures, |_| launches += 1)
        .await;
    assert_eq!(
        launches, 0,
        "quiet source observation does not launch another drain"
    );
}

#[tokio::test]
async fn background_policy_disable_preserves_backlog_and_skips_disabled_bytes() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let prime_offset = capture_state(&state, &snapshot).committed_offset;
    append_records(&path, 1);
    state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .conversation_logging =
        wardian_core::conversations::AgentConversationLoggingSetting::Disabled;
    dispatch_ordinary_capture(&state).await;
    let disabled = capture_state(&state, &snapshot);
    assert_eq!(
        disabled.committed_offset, prime_offset,
        "disabled worker does not drain enabled backlog"
    );
    assert_eq!(
        disabled.reason.as_deref(),
        Some("provider_log_logging_disabled")
    );
    let disabled_start = disabled.open_disabled_from.unwrap();
    append_records(&path, 2);
    let disabled_end = std::fs::metadata(&path).unwrap().len();
    state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .conversation_logging =
        wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
    dispatch_ordinary_capture(&state).await;
    let enabled = capture_state(&state, &snapshot);
    assert_eq!(enabled.committed_offset, disabled_end);
    assert!(enabled
        .disabled_spans
        .iter()
        .any(|span| span.start == disabled_start && span.end == disabled_end));
    let events = state
        .conversation_archive
        .chat_events_for_agent(&session_id)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event
                .text
                .as_deref()
                .is_some_and(|text| text.starts_with("ordinary Off append ")))
            .count(),
        1
    );
}

#[tokio::test]
async fn cancelled_background_owner_retries_on_unchanged_parser_mtime() {
    use std::future::Future;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    append_records(&path, 1);
    let (_, follow_up) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    let request = follow_up.background_captures.into_iter().next().unwrap();
    let claim = state.background_capture.admit(request, false).unwrap();
    let policy = state.conversation_capture_policy_lock.lock().await;
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state, claim,
    ));
    futures_util::future::poll_fn(|cx| match worker.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("held policy gate must block this pass"),
    })
    .await;
    drop(worker);
    drop(policy);
    let parser_mtime = *state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .log_last_modified
        .lock()
        .unwrap();
    dispatch_ordinary_capture(&state).await;
    assert_eq!(
        *state
            .agents
            .lock()
            .await
            .get(&session_id)
            .unwrap()
            .log_last_modified
            .lock()
            .unwrap(),
        parser_mtime
    );
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    assert_eq!(
        capture_state(&state, &snapshot).committed_offset,
        std::fs::metadata(path).unwrap().len()
    );
}

#[tokio::test]
async fn archive_io_error_releases_owner_and_retries_without_source_change() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    let capture_path = home
        .path()
        .join("agents")
        .join(&session_id)
        .join("conversation-capture.json");
    let previous = std::fs::read(&capture_path).unwrap();
    append_records(&path, 1);
    std::fs::write(&capture_path, b"{").unwrap();
    dispatch_ordinary_capture(&state).await;
    let parser_mtime = *state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .log_last_modified
        .lock()
        .unwrap();
    std::fs::write(&capture_path, previous).unwrap();
    dispatch_ordinary_capture(&state).await;
    assert_eq!(
        *state
            .agents
            .lock()
            .await
            .get(&session_id)
            .unwrap()
            .log_last_modified
            .lock()
            .unwrap(),
        parser_mtime
    );
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    assert_eq!(
        capture_state(&state, &snapshot).committed_offset,
        std::fs::metadata(path).unwrap().len()
    );
}

#[tokio::test]
async fn queued_owner_revalidates_replacement_incarnation_after_policy_gate() {
    use std::future::Future;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let old_offset = capture_state(&state, &snapshot).committed_offset;
    append_records(&path, 1);
    let (_, follow_up) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    let claim = state
        .background_capture
        .admit(
            follow_up.background_captures.into_iter().next().unwrap(),
            false,
        )
        .unwrap();
    let policy = state.conversation_capture_policy_lock.lock().await;
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state, claim,
    ));
    futures_util::future::poll_fn(|cx| match worker.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("held policy gate must block this pass"),
    })
    .await;
    let replacement = super::tests::test_active_agent(&session_id, "codex", "Off", None);
    let config = {
        let mut config = replacement.config.lock().unwrap();
        config.session_name = session_id.clone();
        config.is_off = true;
        config.resume_session = snapshot.resume_session.clone();
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    crate::commands::agent::provider_log_tests::register_capture_test_agent(
        &state,
        &config,
        replacement,
    )
    .await
    .unwrap();
    drop(policy);
    worker.await;
    assert_eq!(
        capture_state(&state, &snapshot).committed_offset,
        old_offset
    );
    dispatch_ordinary_capture(&state).await;
    assert_eq!(
        capture_state(&state, &snapshot).committed_offset,
        std::fs::metadata(path).unwrap().len()
    );
}

#[tokio::test]
async fn replaced_source_preserves_existing_incomplete_reason_and_cursor() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let old = capture_state(&state, &snapshot);
    let retired_path = path.with_extension("retired");
    std::fs::rename(&path, &retired_path).unwrap();
    std::fs::copy(&retired_path, &path).unwrap();
    append_records(&path, 1);
    dispatch_ordinary_capture(&state).await;
    let replaced = capture_state(&state, &snapshot);
    assert_eq!(replaced.committed_offset, old.committed_offset);
    assert_eq!(replaced.native_identity, old.native_identity);
    assert_eq!(replaced.status, "incomplete");
    assert_eq!(
        replaced.reason.as_deref(),
        Some("provider_log_source_replaced")
    );
}

#[tokio::test]
async fn disabled_after_admission_then_enabled_before_tick_preserves_unchanged_backlog() {
    use std::future::Future;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    append_records(&path, 1);
    let (_, follow_up) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    let claim = crate::commands::background_capture::admit_background_capture(
        &state,
        follow_up.background_captures.into_iter().next().unwrap(),
        false,
    )
    .await
    .unwrap();
    let policy = state.conversation_capture_policy_lock.lock().await;
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state, claim,
    ));
    futures_util::future::poll_fn(|cx| match worker.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("policy gate must block admitted pass"),
    })
    .await;
    state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .conversation_logging =
        wardian_core::conversations::AgentConversationLoggingSetting::Disabled;
    drop(policy);
    worker.await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let suspended = capture_state(&state, &snapshot);
    assert!(suspended.committed_offset < std::fs::metadata(&path).unwrap().len());
    state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .conversation_logging =
        wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
    dispatch_ordinary_capture(&state).await;
    assert_eq!(
        capture_state(&state, &snapshot).committed_offset,
        std::fs::metadata(path).unwrap().len()
    );
}

#[tokio::test]
async fn stale_telemetry_dispatch_cannot_evict_a_new_incarnation_owner() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    append_records(&path, 1);
    let (sample_ready, sample_ready_rx) = tokio::sync::oneshot::channel();
    let (release_sample, release_sample_rx) = std::sync::mpsc::channel();
    let old_state = state.clone();
    let old_collect = tokio::spawn(async move {
        collect_agent_metrics_with_sampler(&old_state, move |system, sessions, roots| {
            sample_ready.send(()).unwrap();
            release_sample_rx.recv().unwrap();
            stopped_sample(system, sessions, roots)
        })
        .await
    });
    sample_ready_rx.await.unwrap();
    let replacement = super::tests::test_active_agent(&session_id, "codex", "Off", None);
    let config = {
        let agents = state.agents.lock().await;
        let config = agents
            .get(&session_id)
            .unwrap()
            .config
            .lock()
            .unwrap()
            .clone();
        *replacement.config.lock().unwrap() = config.clone();
        config
    };
    crate::commands::agent::provider_log_tests::register_capture_test_agent(
        &state,
        &config,
        replacement,
    )
    .await
    .unwrap();
    let (_, current) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    let request = current.background_captures.into_iter().next().unwrap();
    let new_claim = crate::commands::background_capture::admit_background_capture(
        &state,
        request.clone(),
        false,
    )
    .await
    .unwrap();
    release_sample.send(()).unwrap();
    let (_, stale) = old_collect.await.unwrap();
    let mut stale_launches = 0;
    status::dispatch_capture_follow_up(&state, stale.background_captures, |_| stale_launches += 1)
        .await;
    assert_eq!(stale_launches, 0);
    assert!(
        crate::commands::background_capture::admit_background_capture(&state, request, false)
            .await
            .is_none(),
        "new claim still owns admission"
    );
    crate::commands::background_capture::run_background_capture(&state, new_claim).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    assert_eq!(
        capture_state(&state, &snapshot).committed_offset,
        std::fs::metadata(path).unwrap().len()
    );
}

#[tokio::test]
async fn failed_source_observation_cannot_fall_back_across_replacement() {
    use std::future::Future;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    std::fs::rename(&path, path.with_extension("retired")).unwrap();
    let request = crate::commands::chat::background_capture_request(&state, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(request.source.is_none());
    assert!(
        request.source_path.is_some(),
        "failed observation remains a bound source request"
    );
    let claim =
        crate::commands::background_capture::admit_background_capture(&state, request, true)
            .await
            .unwrap();
    let policy = state.conversation_capture_policy_lock.lock().await;
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state, claim,
    ));
    futures_util::future::poll_fn(|cx| match worker.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("policy gate must block source retry"),
    })
    .await;
    let new_conversation = "replacement-source-conversation";
    let new_path = path.with_file_name(format!(
        "rollout-2026-10-04T00-00-00-{new_conversation}.jsonl"
    ));
    std::fs::write(
        &new_path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session_meta", "payload": {"id": new_conversation, "cwd": home.path()}
            })
        ),
    )
    .unwrap();
    let replacement = super::tests::test_active_agent(&session_id, "codex", "Off", None);
    let config = {
        let mut config = replacement.config.lock().unwrap();
        config.session_name = session_id.clone();
        config.is_off = true;
        config.resume_session = Some(new_conversation.into());
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    crate::commands::agent::provider_log_tests::register_capture_test_agent(
        &state,
        &config,
        replacement,
    )
    .await
    .unwrap();
    let _ = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    drop(policy);
    worker.await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(
        state
            .conversation_archive
            .provider_log_capture_state(
                &session_id,
                context.provider_source_key.as_deref().unwrap(),
            )
            .unwrap()
            .is_none(),
        "old failed observation must not acquire the replacement source"
    );
}

#[tokio::test]
async fn queued_fresh_conversation_revalidates_when_resume_id_is_absent() {
    use std::future::Future;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, _) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    {
        let agents = state.agents.lock().await;
        let mut config = agents.get(&session_id).unwrap().config.lock().unwrap();
        config.resume_session = None;
        config.fresh_provider_session_id = Some("original-fresh-conversation".into());
    }
    let request = crate::commands::chat::background_capture_request(&state, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        request.conversation.as_deref(),
        Some("original-fresh-conversation")
    );
    let claim =
        crate::commands::background_capture::admit_background_capture(&state, request, true)
            .await
            .unwrap();
    let policy = state.conversation_capture_policy_lock.lock().await;
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state, claim,
    ));
    futures_util::future::poll_fn(|cx| match worker.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("policy gate must block original fresh identity"),
    })
    .await;
    state
        .agents
        .lock()
        .await
        .get(&session_id)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .fresh_provider_session_id = Some("replacement-fresh-conversation".into());
    drop(policy);
    worker.await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(
        state
            .conversation_archive
            .provider_log_capture_state(
                &session_id,
                context.provider_source_key.as_deref().unwrap(),
            )
            .unwrap()
            .is_none(),
        "old claim cannot publish under replacement fresh conversation"
    );
}
